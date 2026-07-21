#![deny(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek as _, Write as _};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use libpff_sys::{
    CatalogEvent, CatalogProvenance, CatalogSink, NamedPropertyIdentity, PayloadRequest,
    PropertyDescriptor, PropertyOwner, RecoveryUnit,
};
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

const JOB_SCHEMA_VERSION: i64 = 19;
const INLINE_BLOB_MAX_BYTES: u64 = 64 * 1024;
const INLINE_CACHE_DIRECTORY: &str = ".pstforge-inline-cache";
const PAYLOAD_PACK_FILENAME: &str = "payload.pack";
pub const CANDIDATE_CHECKPOINT_BATCH: u32 = 128;
const DIRECT_SQLITE_CACHE_KIB: u64 = 512 * 1024;
const MAX_RECOVERY_LOG_BYTES: usize = 4 * 1024 * 1024;
const MAX_REPORT_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum JobError {
    #[error("job directory already exists and is not empty: {0}")]
    ExistingJob(PathBuf),
    #[error("unsafe or replaced job path: {0}")]
    UnsafePath(PathBuf),
    #[error("job ledger integrity check failed: {0}")]
    Integrity(String),
    #[error("resume configuration does not match the existing job: {0}")]
    ResumeMismatch(&'static str),
    #[error("job report is unavailable: {0}")]
    ReportUnavailable(&'static str),
    #[error("output part name conflicts with existing data: {0}")]
    OutputNameConflict(PathBuf),
    #[error("job validation was interrupted")]
    Interrupted,
    #[error("invalid catalog event sequence: {0}")]
    EventSequence(String),
    #[error("blob length mismatch: expected {expected}, wrote {actual}")]
    BlobLength { expected: u64, actual: u64 },
    #[error("filesystem operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error("cannot serialize private candidate metadata: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct JobSummary {
    pub committed_candidates: u64,
    pub recovered_candidates: u64,
    pub orphan_candidates: u64,
    pub fragment_candidates: u64,
    pub complete_candidates: u64,
    pub partial_candidates: u64,
    pub unsupported_candidates: u64,
    pub blob_count: u64,
    pub blob_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateRejectionCategory {
    SourceItemUnsupported,
    MalformedCandidate,
    MalformedProperty,
    WriterInputRejected,
    ItemGraphDependencyRejected,
    UnsupportedEmbeddedItem,
    StrandedEmbeddedItem,
}

pub type CandidateRejectionCounts = BTreeMap<CandidateRejectionCategory, u64>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateRejectionMetadata {
    schema_version: u32,
    category: CandidateRejectionCategory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayCandidate {
    pub item_key: String,
    pub id: u32,
    pub provenance: CatalogProvenance,
    pub recovery_index: Option<u64>,
    pub occurrence: u32,
    pub metadata: serde_json::Value,
    pub unit: Option<RecoveryUnit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpooledFolder {
    pub address: Option<libpff_sys::FolderAddress>,
    pub source_id: u32,
    pub parent_source_id: Option<u32>,
    pub name: Option<String>,
    pub container_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpooledBlob {
    pub sha256: String,
    pub byte_len: u64,
    pub pack_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpooledEvent {
    pub sequence: u64,
    pub kind: String,
    pub metadata: serde_json::Value,
    pub blob: Option<SpooledBlob>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpooledCandidate {
    pub item_key: String,
    pub provenance: CatalogProvenance,
    pub source_node_id: Option<u32>,
    pub recovery_index: Option<u64>,
    pub occurrence: u32,
    pub parent_item_key: Option<String>,
    pub parent_attachment_index: Option<u32>,
    pub unit: Option<RecoveryUnit>,
    pub completeness: String,
    pub metadata: serde_json::Value,
    pub events: Vec<SpooledEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateOwnership {
    pub item_key: String,
    pub source_node_id: Option<u32>,
    pub writable: bool,
    pub parent_item_key: Option<String>,
    pub parent_attachment_index: Option<u32>,
    pub embedded_path: Vec<u32>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectTopLevelCandidate {
    pub rowid: i64,
    pub item_key: String,
    pub provenance: CatalogProvenance,
    pub source_node_id: Option<u32>,
    pub recovery_index: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectEmbeddedCandidate {
    pub item_key: String,
    pub provenance: CatalogProvenance,
    pub source_node_id: Option<u32>,
    pub recovery_index: Option<u64>,
    pub parent_item_key: String,
    pub parent_attachment_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpooledCandidateTree {
    pub candidates: Vec<SpooledCandidate>,
    pub ownerships: Vec<CandidateOwnership>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpooledCandidateHeaderPage {
    pub next_rowid: i64,
    pub tree: SpooledCandidateTree,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublishedPart {
    pub index: u32,
    pub filename: String,
    pub byte_len: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub oversize: bool,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ReconstructedField {
    FolderClass,
    MessageClass,
    Subject,
    SenderName,
    SenderAddress,
    MessageFlags,
    InternetCodepage,
    SubmitTime,
    DeliveryTime,
    CreationTime,
    ModificationTime,
    AssociatedDisplayName,
    RecipientDisplayName,
    RecipientAddress,
    AttachmentFilename,
    AttachmentMimeType,
    AttachmentRenderingPosition,
    AttachmentFlags,
    DocumentAttachment,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconstructionCounts {
    pub derived: BTreeMap<ReconstructedField, u64>,
    pub generated: BTreeMap<ReconstructedField, u64>,
}

impl ReconstructionCounts {
    pub fn record_derived(&mut self, field: ReconstructedField) {
        increment_reconstruction(&mut self.derived, field);
    }

    pub fn record_generated(&mut self, field: ReconstructedField) {
        increment_reconstruction(&mut self.generated, field);
    }

    pub fn merge(&mut self, other: Self) {
        merge_reconstructions(&mut self.derived, other.derived);
        merge_reconstructions(&mut self.generated, other.generated);
    }

    pub fn is_empty(&self) -> bool {
        self.derived.is_empty() && self.generated.is_empty()
    }
}

fn increment_reconstruction(
    counts: &mut BTreeMap<ReconstructedField, u64>,
    field: ReconstructedField,
) {
    let count = counts.entry(field).or_default();
    *count = count.saturating_add(1);
}

fn merge_reconstructions(
    counts: &mut BTreeMap<ReconstructedField, u64>,
    additions: BTreeMap<ReconstructedField, u64>,
) {
    for (field, addition) in additions {
        let count = counts.entry(field).or_default();
        *count = count.saturating_add(addition);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartSidecar {
    pub schema_version: String,
    pub producer_version: String,
    pub index: u32,
    pub filename: String,
    pub byte_len: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_device: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_inode: Option<u64>,
    pub store_record_key: String,
    pub folder_count: u64,
    pub message_count: u64,
    pub oversize: bool,
    pub partial: bool,
    pub omitted_folders: u64,
    pub omitted_properties: u64,
    pub omitted_attachments: u64,
    pub reconstructions: ReconstructionCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerEvent {
    pub kind: String,
    pub attempt: u32,
    pub category: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSourceIdentity {
    pub canonical_path: String,
    pub device: u64,
    pub inode: u64,
    pub size_bytes: u64,
    pub modified_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobConfiguration {
    pub tool_compatibility_major: u64,
    pub split_schema_version: String,
    #[serde(default = "default_execution_mode")]
    pub execution_mode: String,
    pub recovery_mode: String,
    pub maximum_pst_bytes: u64,
    pub part_size_policy: String,
    pub writer_format: String,
}

fn default_execution_mode() -> String {
    "restartable".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCompletion {
    pub normal_items: u64,
    pub recovered_items: u64,
    pub orphan_items: u64,
    pub fragment_items: u64,
    pub issues: u64,
    pub issues_dropped: u64,
    pub peak_worker_rss_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPartRecord {
    pub part: PublishedPart,
    pub sidecar: PartSidecar,
    pub item_count: u64,
}

#[derive(Debug)]
pub struct ValidatedReportSnapshot {
    pub json: String,
    pub parts: Vec<PublishedPartRecord>,
    pub evidence: ReportLedgerEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PayloadPackMetrics {
    pub current_bytes: u64,
    pub peak_bytes: u64,
    pub bytes_written: u64,
}

pub struct DurableCatalogSink {
    connection: Connection,
    _parent_directory: File,
    _job_directory: File,
    _private_directory: File,
    _spool_directory: File,
    _partial_directory: File,
    _manifests_directory: File,
    _parts_directory: File,
    private_root: PathBuf,
    spool: PathBuf,
    payload_pack_path: PathBuf,
    payload_pack: File,
    payload_pack_position: Cell<u64>,
    payload_pack_bytes_written: Cell<u64>,
    payload_pack_peak_bytes: Cell<u64>,
    partial: PathBuf,
    manifests: PathBuf,
    parts: PathBuf,
    inline_cache_directory: RefCell<Option<File>>,
    batch_open: Cell<bool>,
    batch_candidates: Cell<u32>,
    batch_pack_start: Cell<u64>,
    active: Option<ActiveCandidate>,
    property: Option<ActiveProperty>,
    pending_named_property: Option<(PropertyDescriptor, NamedPropertyIdentity)>,
    attachment: Option<ActiveAttachment>,
    unit: Option<RecoveryUnit>,
    recent_candidates: HashMap<Vec<u32>, String>,
    replayed_source_ids: HashMap<u32, u32>,
    capture_mode: CatalogCaptureMode,
    next_direct_blob_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogCaptureMode {
    Full,
    Bounded {
        property_prefix_bytes: u64,
        attachment_prefix_bytes: u64,
    },
}

struct ActiveCandidate {
    key: String,
    message_id: u32,
    pack_start: u64,
    sequence: u64,
    recipients: HashSet<u32>,
    supported: bool,
    embedded_path: Vec<u32>,
}

struct CandidateStart {
    metadata: serde_json::Value,
    id: u32,
    provenance: CatalogProvenance,
    recovery_index: Option<u64>,
    parent_message_id: Option<u32>,
    parent_attachment_index: Option<u32>,
    embedded_path: Vec<u32>,
    supported: bool,
}

struct ActiveProperty {
    descriptor: PropertyDescriptor,
    named_property: Option<NamedPropertyIdentity>,
    blob: BlobWriter,
    record: bool,
}

struct ActiveAttachment {
    message_id: u32,
    index: u32,
    attachment_type: Option<i32>,
    expected: Option<u64>,
    blob: Option<BlobWriter>,
}

struct BlobWriter {
    start_offset: u64,
    hasher: Sha256,
    bytes: u64,
    inline: Option<Vec<u8>>,
}

impl DurableCatalogSink {
    pub fn validate_resume(
        job_directory: &Path,
        source: &JobSourceIdentity,
        configuration: &JobConfiguration,
    ) -> Result<(), JobError> {
        let parent_path = job_parent(job_directory);
        let parent_directory = open_directory(parent_path)?;
        let job_name = job_directory
            .file_name()
            .ok_or_else(|| JobError::UnsafePath(job_directory.to_path_buf()))?;
        let held_job_path = fd_path(parent_directory.as_raw_fd()).join(job_name);
        let job_handle = open_directory(&held_job_path)?;
        let held_job_path = fd_path(job_handle.as_raw_fd());
        validate_private_directory(&job_handle, &held_job_path)?;
        let private_directory = open_directory(&held_job_path.join(".pstforge"))?;
        let private_root = fd_path(private_directory.as_raw_fd());
        validate_private_directory(&private_directory, &private_root)?;
        let database = private_root.join("job.sqlite3");
        validate_private_file(&database, true)?;
        let connection = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let schema = read_schema_version(&connection)?;
        if schema != JOB_SCHEMA_VERSION {
            return Err(JobError::ResumeMismatch("job schema version"));
        }
        let integrity = connection.query_row("PRAGMA integrity_check(1)", [], |row| {
            row.get::<_, String>(0)
        })?;
        if integrity != "ok" {
            return Err(JobError::Integrity(integrity));
        }
        validate_resume_metadata(&connection, source, configuration)?;
        validate_foreign_keys(&connection)?;
        read_candidate_rejection_counts(&connection)?;
        Ok(())
    }

    pub fn create(job_directory: &Path) -> Result<Self, JobError> {
        Self::create_with_capture(job_directory, CatalogCaptureMode::Full)
    }

    pub fn create_direct_metadata(
        job_directory: &Path,
        property_prefix_bytes: u64,
        attachment_prefix_bytes: u64,
    ) -> Result<Self, JobError> {
        Self::create_with_capture(
            job_directory,
            CatalogCaptureMode::Bounded {
                property_prefix_bytes,
                attachment_prefix_bytes,
            },
        )
    }

    fn create_with_capture(
        job_directory: &Path,
        capture_mode: CatalogCaptureMode,
    ) -> Result<Self, JobError> {
        let parent_path = job_parent(job_directory);
        let parent_directory = open_directory(parent_path)?;
        let job_name = job_directory
            .file_name()
            .ok_or_else(|| JobError::UnsafePath(job_directory.to_path_buf()))?;
        let held_job_path = fd_path(parent_directory.as_raw_fd()).join(job_name);
        match held_job_path.symlink_metadata() {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(JobError::UnsafePath(job_directory.to_path_buf()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&held_job_path).map_err(|source| io_error(job_directory, source))?;
                sync_file(&parent_directory, parent_path)?;
            }
            Err(source) => return Err(io_error(job_directory, source)),
        }
        let job_handle = open_directory(&held_job_path)?;
        let held_job_path = fd_path(job_handle.as_raw_fd());
        let mut entries =
            fs::read_dir(&held_job_path).map_err(|source| io_error(job_directory, source))?;
        if entries
            .next()
            .transpose()
            .map_err(|source| io_error(job_directory, source))?
            .is_some()
        {
            return Err(JobError::ExistingJob(job_directory.to_path_buf()));
        }
        set_mode(&held_job_path, 0o700)?;
        validate_private_directory(&job_handle, &held_job_path)?;
        let private_root = held_job_path.join(".pstforge");
        let spool = private_root.join("spool");
        let partial = private_root.join("partial");
        let manifests = private_root.join("manifests");
        for directory in [&private_root, &spool, &partial, &manifests] {
            fs::create_dir(directory).map_err(|source| io_error(directory, source))?;
            set_mode(directory, 0o700)?;
        }
        let parts = held_job_path.join("parts");
        fs::create_dir(&parts).map_err(|source| io_error(&parts, source))?;
        set_mode(&parts, 0o700)?;
        let private_directory = open_directory(&private_root)?;
        let private_root = fd_path(private_directory.as_raw_fd());
        let spool_directory = open_directory(&private_root.join("spool"))?;
        let spool = fd_path(spool_directory.as_raw_fd());
        let payload_pack_path = spool.join(PAYLOAD_PACK_FILENAME);
        let payload_pack = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&payload_pack_path)
            .map_err(|source| io_error(&payload_pack_path, source))?;
        let partial_directory = open_directory(&private_root.join("partial"))?;
        let partial = fd_path(partial_directory.as_raw_fd());
        let manifests_directory = open_directory(&private_root.join("manifests"))?;
        let manifests = fd_path(manifests_directory.as_raw_fd());
        let parts_directory = open_directory(&parts)?;
        let parts = fd_path(parts_directory.as_raw_fd());
        let database = private_root.join("job.sqlite3");
        let connection = Connection::open_with_flags(
            &database,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        set_mode(&database, 0o600)?;
        configure(&connection)?;
        configure_capture_mode(&connection, capture_mode)?;
        create_schema(&connection)?;
        connection.execute(
            "INSERT INTO job_metadata(key, value) VALUES ('schema_version', ?1)",
            [JOB_SCHEMA_VERSION.to_string()],
        )?;
        secure_sqlite_files(&private_root)?;
        sync_file(&private_directory, &private_root)?;
        sync_file(&job_handle, &held_job_path)?;
        Ok(Self {
            connection,
            _parent_directory: parent_directory,
            _job_directory: job_handle,
            _private_directory: private_directory,
            _spool_directory: spool_directory,
            _partial_directory: partial_directory,
            _manifests_directory: manifests_directory,
            _parts_directory: parts_directory,
            private_root,
            spool,
            payload_pack_path,
            payload_pack,
            payload_pack_position: Cell::new(0),
            payload_pack_bytes_written: Cell::new(0),
            payload_pack_peak_bytes: Cell::new(0),
            partial,
            manifests,
            parts,
            inline_cache_directory: RefCell::new(None),
            batch_open: Cell::new(false),
            batch_candidates: Cell::new(0),
            batch_pack_start: Cell::new(0),
            active: None,
            property: None,
            pending_named_property: None,
            attachment: None,
            unit: None,
            recent_candidates: HashMap::new(),
            replayed_source_ids: HashMap::new(),
            capture_mode,
            next_direct_blob_id: 1,
        })
    }

    pub fn open(job_directory: &Path) -> Result<Self, JobError> {
        Self::open_expected(job_directory, None, None)
    }

    pub fn open_interruptible(
        job_directory: &Path,
        interrupted: &AtomicBool,
    ) -> Result<Self, JobError> {
        Self::open_expected(job_directory, None, Some(interrupted))
    }

    pub fn open_resume(
        job_directory: &Path,
        source: &JobSourceIdentity,
        configuration: &JobConfiguration,
    ) -> Result<Self, JobError> {
        Self::open_expected(job_directory, Some((source, configuration)), None)
    }

    pub fn open_resume_interruptible(
        job_directory: &Path,
        source: &JobSourceIdentity,
        configuration: &JobConfiguration,
        interrupted: &AtomicBool,
    ) -> Result<Self, JobError> {
        Self::open_expected(
            job_directory,
            Some((source, configuration)),
            Some(interrupted),
        )
    }

    pub fn enable_direct_metadata_capture(
        &mut self,
        property_prefix_bytes: u64,
        attachment_prefix_bytes: u64,
    ) -> Result<(), JobError> {
        if self.active.is_some() || self.property.is_some() || self.attachment.is_some() {
            return Err(JobError::EventSequence(
                "cannot change capture mode during an active candidate".to_owned(),
            ));
        }
        let mut statement = self.connection.prepare(
            "SELECT metadata_json FROM candidate_events \
             WHERE kind IN ('property_direct', 'attachment_direct')",
        )?;
        let mut rows = statement.query([])?;
        let mut maximum_id = 0_u64;
        while let Some(row) = rows.next()? {
            let metadata: serde_json::Value = serde_json::from_str(&row.get::<_, String>(0)?)?;
            let id = metadata
                .get("direct_id")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| {
                    JobError::Integrity("direct metadata event has no stream identifier".to_owned())
                })?;
            maximum_id = maximum_id.max(id);
        }
        self.next_direct_blob_id = maximum_id
            .checked_add(1)
            .ok_or_else(|| JobError::Integrity("direct blob identifier overflow".to_owned()))?;
        self.capture_mode = CatalogCaptureMode::Bounded {
            property_prefix_bytes,
            attachment_prefix_bytes,
        };
        configure_capture_mode(&self.connection, self.capture_mode)?;
        Ok(())
    }

    fn open_expected(
        job_directory: &Path,
        expected: Option<(&JobSourceIdentity, &JobConfiguration)>,
        interrupted: Option<&AtomicBool>,
    ) -> Result<Self, JobError> {
        let parent_path = job_parent(job_directory);
        let parent_directory = open_directory(parent_path)?;
        let job_name = job_directory
            .file_name()
            .ok_or_else(|| JobError::UnsafePath(job_directory.to_path_buf()))?;
        let held_job_path = fd_path(parent_directory.as_raw_fd()).join(job_name);
        let job_handle = open_directory(&held_job_path)?;
        let held_job_path = fd_path(job_handle.as_raw_fd());
        validate_private_directory(&job_handle, &held_job_path)?;
        let private_directory = open_directory(&held_job_path.join(".pstforge"))?;
        let private_root = fd_path(private_directory.as_raw_fd());
        validate_private_directory(&private_directory, &private_root)?;
        let spool_directory = open_directory(&private_root.join("spool"))?;
        let spool = fd_path(spool_directory.as_raw_fd());
        validate_private_directory(&spool_directory, &spool)?;
        let parts_directory = open_directory(&held_job_path.join("parts"))?;
        let parts = fd_path(parts_directory.as_raw_fd());
        validate_private_directory(&parts_directory, &parts)?;
        let partial_directory = open_directory(&private_root.join("partial"))?;
        validate_private_directory(&partial_directory, &private_root.join("partial"))?;
        let partial = fd_path(partial_directory.as_raw_fd());
        let manifests_directory = open_directory(&private_root.join("manifests"))?;
        validate_private_directory(&manifests_directory, &private_root.join("manifests"))?;
        let manifests = fd_path(manifests_directory.as_raw_fd());
        let database = private_root.join("job.sqlite3");
        validate_private_file(&database, true)?;
        validate_private_file(&private_root.join("job.sqlite3-wal"), false)?;
        validate_private_file(&private_root.join("job.sqlite3-shm"), false)?;
        validate_private_root_entries(&private_root)?;
        let mut connection =
            Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.execute_batch("PRAGMA trusted_schema = OFF; PRAGMA busy_timeout = 5000;")?;
        if expected.is_some() {
            connection.execute_batch("PRAGMA query_only = ON;")?;
        }
        validate_private_file(&database, true)?;
        validate_private_file(&private_root.join("job.sqlite3-wal"), false)?;
        validate_private_file(&private_root.join("job.sqlite3-shm"), false)?;
        let schema = read_schema_version(&connection)?;
        if schema != JOB_SCHEMA_VERSION {
            return Err(if expected.is_some() {
                JobError::ResumeMismatch("job schema version")
            } else {
                JobError::Integrity(format!("unsupported schema version {schema}"))
            });
        }
        if let Some((source, configuration)) = expected {
            validate_resume_metadata(&connection, source, configuration)?;
        }
        let integrity = run_sql_interruptible(&mut connection, interrupted, |connection| {
            Ok(
                connection.query_row("PRAGMA integrity_check(1)", [], |row| {
                    row.get::<_, String>(0)
                })?,
            )
        })?;
        if integrity != "ok" {
            return Err(JobError::Integrity(integrity));
        }
        validate_foreign_keys(&connection)?;
        read_candidate_rejection_counts(&connection)?;
        if expected.is_some() {
            connection.execute_batch("PRAGMA query_only = OFF;")?;
            invalidate_report_snapshot(&connection)?;
        }
        configure(&connection)?;
        run_sql_interruptible(&mut connection, interrupted, |connection| {
            ensure_inline_blob_table(connection)
        })?;
        run_sql_interruptible(&mut connection, interrupted, |connection| {
            ensure_pack_offset_column(connection)
        })?;
        let payload_pack_path = spool.join(PAYLOAD_PACK_FILENAME);
        let pack_exists = match payload_pack_path.symlink_metadata() {
            Ok(_) => {
                validate_private_file(&payload_pack_path, true)?;
                true
            }
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(source) => return Err(io_error(&payload_pack_path, source)),
        };
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        if pack_exists {
            options.create(false);
        } else {
            options.create_new(true);
        }
        let mut payload_pack = options
            .open(&payload_pack_path)
            .map_err(|source| io_error(&payload_pack_path, source))?;
        reconcile_payload_pack(&connection, &mut payload_pack, &payload_pack_path)?;
        let payload_pack_position = payload_pack
            .metadata()
            .map_err(|source| io_error(&payload_pack_path, source))?
            .len();
        run_sql_interruptible(&mut connection, interrupted, |connection| {
            reconcile_publications(
                connection,
                &partial_directory,
                &manifests_directory,
                &partial,
                &parts,
                &manifests,
                interrupted,
            )
        })?;
        remove_stale_partials(&partial_directory, &partial, interrupted)?;
        run_sql_interruptible(&mut connection, interrupted, |connection| {
            validate_foreign_keys(connection)
        })?;
        validate_blob_store(&connection, &spool, interrupted)?;
        validate_part_store(&connection, &parts, &manifests, interrupted)?;
        remove_temporary_blobs(&spool, interrupted)?;
        remove_unreferenced_blobs(&connection, &spool, interrupted)?;
        Ok(Self {
            connection,
            _parent_directory: parent_directory,
            _job_directory: job_handle,
            _private_directory: private_directory,
            _spool_directory: spool_directory,
            _partial_directory: partial_directory,
            _manifests_directory: manifests_directory,
            _parts_directory: parts_directory,
            private_root,
            spool,
            payload_pack_path,
            payload_pack,
            payload_pack_position: Cell::new(payload_pack_position),
            payload_pack_bytes_written: Cell::new(0),
            payload_pack_peak_bytes: Cell::new(payload_pack_position),
            partial,
            manifests,
            parts,
            inline_cache_directory: RefCell::new(None),
            batch_open: Cell::new(false),
            batch_candidates: Cell::new(0),
            batch_pack_start: Cell::new(0),
            active: None,
            property: None,
            pending_named_property: None,
            attachment: None,
            unit: None,
            recent_candidates: HashMap::new(),
            replayed_source_ids: HashMap::new(),
            capture_mode: CatalogCaptureMode::Full,
            next_direct_blob_id: 1,
        })
    }

    pub fn allocated_bytes(&self) -> Result<u64, JobError> {
        Ok(allocated_private_bytes(&self.private_root)?
            .saturating_add(allocated_tracked_part_bytes(&self.connection, &self.parts)?))
    }

    pub fn payload_pack_metrics(&self) -> Result<PayloadPackMetrics, JobError> {
        let current_bytes = self.payload_pack_len()?;
        if current_bytes != self.payload_pack_position.get() {
            return Err(JobError::Integrity(
                "payload pack length disagrees with the append cursor".to_owned(),
            ));
        }
        Ok(PayloadPackMetrics {
            current_bytes,
            peak_bytes: self.payload_pack_peak_bytes.get(),
            bytes_written: self.payload_pack_bytes_written.get(),
        })
    }

    pub fn available_part_filename(&self, part_index: u32) -> Result<String, JobError> {
        let filename = format!("part-{part_index:04}.pst");
        let sidecar = filename.trim_end_matches(".pst").to_owned() + ".json";
        for path in [self.parts.join(&filename), self.manifests.join(sidecar)] {
            if path_exists(&path)? {
                return Err(JobError::OutputNameConflict(path));
            }
        }
        Ok(filename)
    }

    pub fn checkpoint(&self) -> Result<(), JobError> {
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "cannot checkpoint during an active candidate".to_owned(),
            ));
        }
        self.commit_candidate_batch()?;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        sync_directory(&self.private_root)
    }

    pub fn reconcile_pending_publications(&mut self) -> Result<(), JobError> {
        self.commit_candidate_batch()?;
        reconcile_publications(
            &mut self.connection,
            &self._partial_directory,
            &self._manifests_directory,
            &self.partial,
            &self.parts,
            &self.manifests,
            None,
        )
    }

    fn begin_candidate(&self) -> Result<(), JobError> {
        if !self.batch_open.get() {
            self.connection.execute_batch("BEGIN IMMEDIATE")?;
            self.batch_open.set(true);
            self.batch_candidates.set(0);
            self.batch_pack_start.set(self.payload_pack_position.get());
        }
        self.connection
            .execute_batch("SAVEPOINT active_candidate")?;
        Ok(())
    }

    fn finish_candidate(&self) -> Result<(), JobError> {
        self.connection.execute_batch("RELEASE active_candidate")?;
        let candidates = self.batch_candidates.get().saturating_add(1);
        self.batch_candidates.set(candidates);
        if matches!(self.capture_mode, CatalogCaptureMode::Full)
            && candidates >= CANDIDATE_CHECKPOINT_BATCH
        {
            self.commit_candidate_batch()?;
        }
        Ok(())
    }

    fn abort_candidate(&self) -> Result<(), JobError> {
        self.connection
            .execute_batch("ROLLBACK TO active_candidate; RELEASE active_candidate;")?;
        Ok(())
    }

    fn commit_candidate_batch(&self) -> Result<(), JobError> {
        if self.batch_open.get() {
            self.payload_pack
                .sync_all()
                .map_err(|source| io_error(&self.payload_pack_path, source))?;
            if self.payload_pack_len()? != self.payload_pack_position.get() {
                return Err(JobError::Integrity(
                    "payload pack length changed before durable batch commit".to_owned(),
                ));
            }
            self.connection.execute_batch("COMMIT")?;
            self.batch_open.set(false);
            self.batch_candidates.set(0);
            self.batch_pack_start.set(self.payload_pack_position.get());
        }
        Ok(())
    }

    fn payload_pack_len(&self) -> Result<u64, JobError> {
        self.payload_pack
            .metadata()
            .map(|metadata| metadata.len())
            .map_err(|source| io_error(&self.payload_pack_path, source))
    }

    fn new_blob_writer(&mut self) -> Result<BlobWriter, JobError> {
        match self.capture_mode {
            CatalogCaptureMode::Full => Ok(BlobWriter::new(self.payload_pack_position.get())),
            CatalogCaptureMode::Bounded { .. } => Ok(BlobWriter::new_inline()),
        }
    }

    fn truncate_payload_pack(&mut self, length: u64) -> Result<(), JobError> {
        self.payload_pack
            .set_len(length)
            .map_err(|source| io_error(&self.payload_pack_path, source))?;
        self.payload_pack
            .seek(std::io::SeekFrom::Start(length))
            .map_err(|source| io_error(&self.payload_pack_path, source))?;
        self.payload_pack_position.set(length);
        Ok(())
    }

    pub fn finalize_private_work(&mut self, keep_work: bool) -> Result<(), JobError> {
        self.finalize_private_work_expected(keep_work, None)
    }

    pub fn finalize_private_work_interruptible(
        &mut self,
        keep_work: bool,
        interrupted: &AtomicBool,
    ) -> Result<(), JobError> {
        self.finalize_private_work_expected(keep_work, Some(interrupted))
    }

    fn finalize_private_work_expected(
        &mut self,
        keep_work: bool,
        interrupted: Option<&AtomicBool>,
    ) -> Result<(), JobError> {
        check_job_interrupted(interrupted)?;
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "cannot finalize private work during an active candidate".to_owned(),
            ));
        }
        self.commit_candidate_batch()?;
        let unfinished = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM candidates WHERE status IN ('pending', 'spooled', 'failed')) \
             OR EXISTS(SELECT 1 FROM publication_intents)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if unfinished {
            return Err(JobError::EventSequence(
                "cannot remove private work before every candidate is finalized".to_owned(),
            ));
        }
        remove_stale_partials(&self._partial_directory, &self.partial, interrupted)?;
        if keep_work {
            self.connection.execute(
                "INSERT OR REPLACE INTO job_metadata(key, value) VALUES ('work_retained', 'true')",
                [],
            )?;
            return self.checkpoint();
        }
        let (live_blob_count, live_blob_bytes) = self.connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(byte_len), 0) FROM blobs",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
        )?;
        let final_blob_count = read_optional_metadata_u64(&self.connection, "final_blob_count")?;
        let final_blob_bytes = read_optional_metadata_u64(&self.connection, "final_blob_bytes")?;
        let (blob_count, blob_bytes) = match (final_blob_count, final_blob_bytes) {
            (None, None) => (live_blob_count, live_blob_bytes),
            (Some(_), Some(_)) if live_blob_count == 0 && live_blob_bytes == 0 => {
                let compaction_pending = self
                    .connection
                    .query_row(
                        "SELECT value FROM job_metadata \
                         WHERE key = 'cleanup_compaction_pending'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                    .is_some_and(|value| value == "true");
                if compaction_pending {
                    self.compact_private_ledger(interrupted)?;
                }
                remove_spool_contents(&self._spool_directory, &self.spool, interrupted)?;
                self.checkpoint()?;
                return Ok(());
            }
            _ => {
                return Err(JobError::Integrity(
                    "final spool metrics disagree with retained work".to_owned(),
                ));
            }
        };
        run_sql_interruptible(&mut self.connection, interrupted, |connection| {
            connection.execute_batch("PRAGMA secure_delete = ON;")?;
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE candidate_events SET blob_sha256 = NULL, byte_len = NULL \
                 WHERE blob_sha256 IS NOT NULL",
                [],
            )?;
            transaction.execute("DELETE FROM blobs", [])?;
            for (key, value) in [
                ("final_blob_count", blob_count.to_string()),
                ("final_blob_bytes", blob_bytes.to_string()),
                ("work_retained", "false".to_owned()),
                ("cleanup_compaction_pending", "true".to_owned()),
            ] {
                transaction.execute(
                    "INSERT OR REPLACE INTO job_metadata(key, value) VALUES (?1, ?2)",
                    params![key, value],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })?;
        self.compact_private_ledger(interrupted)?;
        remove_spool_contents(&self._spool_directory, &self.spool, interrupted)?;
        Ok(())
    }

    fn compact_private_ledger(&mut self, interrupted: Option<&AtomicBool>) -> Result<(), JobError> {
        self.checkpoint()?;
        if std::env::var_os("PSTFORGE_TEST_LONG_CLEANUP_SQL").is_some() {
            let marker = self.partial.join("cleanup-test-marker.partial");
            fs::write(&marker, b"cleanup SQL active")
                .map_err(|source| io_error(&marker, source))?;
            set_mode(&marker, 0o600)?;
            sync_file(&self._partial_directory, &self.partial)?;
            run_sql_interruptible(&mut self.connection, interrupted, |connection| {
                connection.query_row(
                    "WITH RECURSIVE count(value) AS (\
                         SELECT 1 UNION ALL SELECT value + 1 FROM count WHERE value < 100000000\
                     ) SELECT SUM(value) FROM count",
                    [],
                    |row| row.get::<_, i64>(0),
                )?;
                Ok(())
            })?;
            fs::remove_file(&marker).map_err(|source| io_error(&marker, source))?;
            sync_file(&self._partial_directory, &self.partial)?;
        }
        run_sql_interruptible(&mut self.connection, interrupted, |connection| {
            connection.execute_batch("VACUUM;")?;
            Ok(())
        })?;
        run_sql_interruptible(&mut self.connection, interrupted, |connection| {
            connection.execute(
                "INSERT OR REPLACE INTO job_metadata(key, value) \
                 VALUES ('cleanup_compaction_pending', 'false')",
                [],
            )?;
            Ok(())
        })?;
        self.checkpoint()
    }

    pub fn mark_candidates_unsupported(
        &mut self,
        item_keys: &[String],
        category: CandidateRejectionCategory,
    ) -> Result<(), JobError> {
        let rejections = item_keys
            .iter()
            .cloned()
            .map(|item_key| (item_key, category))
            .collect::<Vec<_>>();
        self.mark_candidate_rejections(&rejections)
    }

    pub fn mark_candidate_rejections(
        &mut self,
        rejections: &[(String, CandidateRejectionCategory)],
    ) -> Result<(), JobError> {
        if self.active.is_some() || rejections.is_empty() {
            return Err(JobError::EventSequence(
                "cannot reject an empty candidate set during an active candidate".to_owned(),
            ));
        }
        self.commit_candidate_batch()?;
        let transaction = self.connection.transaction()?;
        for (item_key, category) in rejections {
            let metadata_json = serde_json::to_string(&CandidateRejectionMetadata {
                schema_version: 1,
                category: *category,
            })?;
            let changed = transaction.execute(
                "UPDATE candidates SET status = 'unsupported' \
                 WHERE item_key = ?1 AND status = 'spooled'",
                [item_key],
            )?;
            if changed != 1 {
                return Err(JobError::Integrity(format!(
                    "candidate {item_key} is not available for rejection"
                )));
            }
            transaction.execute(
                "INSERT INTO candidate_events(\
                    item_key, sequence, kind, metadata_json, blob_sha256, byte_len\
                 ) SELECT ?1, COALESCE(MAX(sequence), 0) + 1, \
                          'output_unrepresentable', ?2, NULL, NULL \
                   FROM candidate_events WHERE item_key = ?1",
                params![item_key, metadata_json],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn mark_stranded_embedded_candidates_unsupported(&mut self) -> Result<u64, JobError> {
        let mut statement = self.connection.prepare(
            "WITH RECURSIVE stranded(item_key) AS (\
                 SELECT child.item_key FROM candidates child \
                 JOIN candidates parent ON parent.item_key = child.parent_item_key \
                 WHERE child.status = 'spooled' \
                   AND parent.status IN ('written', 'unsupported') \
                 UNION \
                 SELECT child.item_key FROM candidates child \
                 JOIN stranded parent ON parent.item_key = child.parent_item_key \
                 WHERE child.status = 'spooled'\
             ) SELECT item_key FROM stranded ORDER BY item_key",
        )?;
        let item_keys = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if item_keys.is_empty() {
            return Ok(0);
        }
        self.mark_candidates_unsupported(
            &item_keys,
            CandidateRejectionCategory::StrandedEmbeddedItem,
        )?;
        Ok(u64::try_from(item_keys.len()).unwrap_or(u64::MAX))
    }

    pub fn candidate_rejection_counts(&self) -> Result<CandidateRejectionCounts, JobError> {
        read_candidate_rejection_counts(&self.connection)
    }

    pub fn abort_worker_attempt(&mut self) -> Result<(), JobError> {
        self.property = None;
        self.attachment = None;
        self.unit = None;
        self.recent_candidates.clear();
        if let Some(active) = self.active.take() {
            self.truncate_payload_pack(active.pack_start)?;
            if let Err(error) = self.abort_candidate() {
                let _ = self.connection.execute_batch("ROLLBACK");
                let _ = self.truncate_payload_pack(self.batch_pack_start.get());
                self.batch_open.set(false);
                self.batch_candidates.set(0);
                return Err(error);
            }
        }
        self.commit_candidate_batch()
    }

    pub fn bind_source(&self, source: &JobSourceIdentity) -> Result<(), JobError> {
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "cannot bind source during an active candidate".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT INTO job_metadata(key, value) VALUES ('source_identity', ?1)",
            [serde_json::to_string(source)?],
        )?;
        Ok(())
    }

    pub fn bind_recovery_mode(&self, mode: &'static str) -> Result<(), JobError> {
        if self.active.is_some() || !matches!(mode, "balanced" | "aggressive") {
            return Err(JobError::EventSequence(
                "cannot bind invalid recovery mode during an active candidate".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT INTO job_metadata(key, value) VALUES ('recovery_mode', ?1)",
            [mode],
        )?;
        Ok(())
    }

    pub fn bind_configuration(&self, configuration: &JobConfiguration) -> Result<(), JobError> {
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "cannot bind configuration during an active candidate".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT INTO job_metadata(key, value) VALUES ('job_configuration', ?1)",
            [serde_json::to_string(configuration)?],
        )?;
        Ok(())
    }

    pub fn record_worker_supervision(
        &self,
        attempts: u32,
        failures: u32,
        exhausted: bool,
    ) -> Result<(), JobError> {
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "cannot record supervision during an active candidate".to_owned(),
            ));
        }
        for (key, value) in [
            ("worker_attempts", attempts.to_string()),
            ("worker_failures", failures.to_string()),
            ("worker_retries_exhausted", exhausted.to_string()),
        ] {
            self.connection.execute(
                "INSERT OR REPLACE INTO job_metadata(key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }
        Ok(())
    }

    pub fn record_interrupted(&self) -> Result<(), JobError> {
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "cannot record interruption during an active candidate".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT OR REPLACE INTO job_metadata(key, value) VALUES ('interrupted', 'true')",
            [],
        )?;
        Ok(())
    }

    pub fn clear_interrupted(&self) -> Result<(), JobError> {
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "cannot clear interruption during an active candidate".to_owned(),
            ));
        }
        self.connection
            .execute("DELETE FROM job_metadata WHERE key = 'interrupted'", [])?;
        Ok(())
    }

    pub fn record_direct_terminal_failure(&self, category: &str) -> Result<(), JobError> {
        if self.active.is_some()
            || !matches!(
                category,
                "worker_crash" | "worker_stall" | "worker_protocol"
            )
        {
            return Err(JobError::EventSequence(
                "cannot record invalid direct terminal failure".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT OR REPLACE INTO job_metadata(key, value) \
             VALUES ('direct_terminal_failure', ?1)",
            [category],
        )?;
        Ok(())
    }

    pub fn worker_supervision(&self) -> Result<(u32, u32), JobError> {
        let attempts = read_optional_metadata_u32(&self.connection, "worker_attempts")?;
        let failures = read_optional_metadata_u32(&self.connection, "worker_failures")?;
        Ok((attempts, failures))
    }

    pub fn worker_retries_exhausted(&self) -> Result<bool, JobError> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM job_metadata WHERE key = 'worker_retries_exhausted'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match value.as_deref() {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            Some(_) => Err(JobError::Integrity(
                "worker retry exhaustion metadata is invalid".to_owned(),
            )),
        }
    }

    pub fn record_recovery_completion(
        &self,
        completion: &RecoveryCompletion,
    ) -> Result<(), JobError> {
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "cannot complete recovery during an active candidate".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT OR REPLACE INTO job_metadata(key, value) VALUES ('recovery_completion', ?1)",
            [serde_json::to_string(completion)?],
        )?;
        Ok(())
    }

    pub fn recovery_completion(&self) -> Result<Option<RecoveryCompletion>, JobError> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM job_metadata WHERE key = 'recovery_completion'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value).map_err(JobError::from))
            .transpose()
    }

    pub fn isolated_units(&self) -> Result<Vec<(RecoveryUnit, u32)>, JobError> {
        let mut statement = self
            .connection
            .prepare("SELECT unit_json, failures FROM isolated_units ORDER BY unit_json")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(unit, failures)| {
                Ok((
                    serde_json::from_str(&unit)?,
                    checked_u32(failures, "isolated unit failures")?,
                ))
            })
            .collect()
    }

    pub fn published_parts(&self) -> Result<Vec<PublishedPartRecord>, JobError> {
        read_published_parts(&self.connection)
    }

    pub fn publish_report_snapshot<T: Serialize>(&self, report: &T) -> Result<(), JobError> {
        if self.active.is_some() || self.property.is_some() || self.attachment.is_some() {
            return Err(JobError::EventSequence(
                "cannot publish a report during an active candidate".to_owned(),
            ));
        }
        let json = serde_json::to_string(report)?;
        if json.len() > MAX_REPORT_SNAPSHOT_BYTES {
            return Err(JobError::Integrity(
                "report snapshot exceeds the supported size".to_owned(),
            ));
        }
        let sha256 = digest_hex(Sha256::digest(json.as_bytes()).as_slice());
        let state_sha256 = report_ledger_digest(&self.connection)?;
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT OR REPLACE INTO job_metadata(key, value) VALUES ('split_report_snapshot', ?1)",
            [&json],
        )?;
        transaction.execute(
            "INSERT OR REPLACE INTO job_metadata(key, value) VALUES ('split_report_snapshot_sha256', ?1)",
            [&sha256],
        )?;
        transaction.execute(
            "INSERT OR REPLACE INTO job_metadata(key, value) VALUES ('split_report_state_sha256', ?1)",
            [&state_sha256],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn candidate_is_terminal(&self, item_key: &str) -> Result<bool, JobError> {
        let status = self
            .connection
            .query_row(
                "SELECT status FROM candidates WHERE item_key = ?1 \
                 AND status IN ('spooled', 'written', 'unsupported')",
                [item_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| {
                JobError::Integrity(format!(
                    "direct worker candidate {item_key} is absent from the catalog"
                ))
            })?;
        Ok(matches!(status.as_str(), "written" | "unsupported"))
    }

    pub fn next_direct_top_level_candidate(
        &self,
        after_rowid: i64,
    ) -> Result<Option<DirectTopLevelCandidate>, JobError> {
        let row = self
            .connection
            .query_row(
                "SELECT rowid, item_key, provenance, source_node_id, recovery_index \
                 FROM candidates WHERE rowid > ?1 AND parent_item_key IS NULL \
                 AND status IN ('spooled', 'written', 'unsupported') \
                 ORDER BY rowid LIMIT 1",
                [after_rowid],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .optional()?;
        row.map(
            |(rowid, item_key, provenance, source_node_id, recovery_index)| {
                Ok(DirectTopLevelCandidate {
                    rowid,
                    item_key,
                    provenance: parse_provenance(&provenance)?,
                    source_node_id: source_node_id
                        .map(|value| checked_u32(value, "candidate source node id"))
                        .transpose()?,
                    recovery_index: recovery_index
                        .map(|value| checked_u64(value, "candidate recovery index"))
                        .transpose()?,
                })
            },
        )
        .transpose()
    }

    pub fn direct_embedded_candidates_interruptible(
        &self,
        root_item_key: &str,
        interrupted: &AtomicBool,
    ) -> Result<Vec<DirectEmbeddedCandidate>, JobError> {
        check_job_interrupted(Some(interrupted))?;
        let mut statement = self.connection.prepare(
            "WITH RECURSIVE tree(item_key) AS (\
                SELECT item_key FROM candidates \
                WHERE item_key = ?1 AND parent_item_key IS NULL \
                  AND status IN ('spooled', 'written', 'unsupported') \
                UNION \
                SELECT child.item_key FROM candidates child \
                JOIN tree parent ON child.parent_item_key = parent.item_key \
                WHERE child.status IN ('spooled', 'written', 'unsupported')\
             ) \
             SELECT child.item_key, child.provenance, child.source_node_id, \
                    child.recovery_index, child.parent_item_key, \
                    child.parent_attachment_index \
             FROM tree member JOIN candidates child ON child.item_key = member.item_key \
             WHERE child.parent_item_key IS NOT NULL \
             ORDER BY child.embedded_path_json, child.item_key",
        )?;
        let mut query = statement.query([root_item_key])?;
        let mut candidates = Vec::new();
        while let Some(row) = query.next()? {
            check_job_interrupted(Some(interrupted))?;
            candidates.push(DirectEmbeddedCandidate {
                item_key: row.get(0)?,
                provenance: parse_provenance(&row.get::<_, String>(1)?)?,
                source_node_id: row
                    .get::<_, Option<i64>>(2)?
                    .map(|value| checked_u32(value, "candidate source node id"))
                    .transpose()?,
                recovery_index: row
                    .get::<_, Option<i64>>(3)?
                    .map(|value| checked_u64(value, "candidate recovery index"))
                    .transpose()?,
                parent_item_key: row.get::<_, Option<String>>(4)?.ok_or_else(|| {
                    JobError::Integrity(
                        "direct embedded candidate has no parent item key".to_owned(),
                    )
                })?,
                parent_attachment_index: row
                    .get::<_, Option<i64>>(5)?
                    .map(|value| checked_u32(value, "parent attachment index"))
                    .transpose()?
                    .ok_or_else(|| {
                        JobError::Integrity(
                            "direct embedded candidate has no parent attachment index".to_owned(),
                        )
                    })?,
            });
        }
        Ok(candidates)
    }

    pub fn record_worker_event(
        &self,
        kind: &'static str,
        attempt: u32,
        category: &'static str,
    ) -> Result<(), JobError> {
        if self.active.is_some() || !matches!(kind, "started" | "failure") {
            return Err(JobError::EventSequence(
                "invalid worker supervision event".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT INTO worker_events(kind, attempt, category) VALUES (?1, ?2, ?3)",
            params![kind, attempt, category],
        )?;
        Ok(())
    }

    pub fn worker_events(&self) -> Result<Vec<WorkerEvent>, JobError> {
        let mut statement = self
            .connection
            .prepare("SELECT kind, attempt, category FROM worker_events ORDER BY sequence")?;
        Ok(statement
            .query_map([], |row| {
                Ok(WorkerEvent {
                    kind: row.get(0)?,
                    attempt: row.get(1)?,
                    category: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn record_isolated_unit(&self, unit: RecoveryUnit, failures: u32) -> Result<(), JobError> {
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "cannot isolate a unit during an active candidate".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT OR REPLACE INTO isolated_units(unit_json, failures) VALUES (?1, ?2)",
            params![serde_json::to_string(&unit)?, failures],
        )?;
        Ok(())
    }

    pub fn summary(&self) -> Result<JobSummary, JobError> {
        let (committed, recovered, orphan, fragment, complete, partial, unsupported) =
            self.connection.query_row(
                "SELECT COUNT(*),\
                    COALESCE(SUM(provenance = 'recovered'), 0),\
                    COALESCE(SUM(provenance = 'orphan'), 0),\
                    COALESCE(SUM(provenance = 'fragment'), 0),\
                    COALESCE(SUM(completeness = 'complete'), 0),\
                    COALESCE(SUM(completeness = 'partial'), 0),\
                    COALESCE(SUM(status = 'unsupported'), 0)\
             FROM candidates WHERE status IN ('spooled', 'written', 'unsupported')",
                [],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, u64>(3)?,
                        row.get::<_, u64>(4)?,
                        row.get::<_, u64>(5)?,
                        row.get::<_, u64>(6)?,
                    ))
                },
            )?;
        let (live_blob_count, live_blob_bytes) = self.connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(byte_len), 0) FROM blobs",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
        )?;
        let final_blob_count = read_optional_metadata_u64(&self.connection, "final_blob_count")?;
        let final_blob_bytes = read_optional_metadata_u64(&self.connection, "final_blob_bytes")?;
        let (blob_count, blob_bytes) = match (final_blob_count, final_blob_bytes) {
            (Some(count), Some(bytes)) if live_blob_count == 0 && live_blob_bytes == 0 => {
                (count, bytes)
            }
            (None, None) => (live_blob_count, live_blob_bytes),
            _ => {
                return Err(JobError::Integrity(
                    "final spool metrics disagree with retained work".to_owned(),
                ));
            }
        };
        Ok(JobSummary {
            committed_candidates: committed,
            recovered_candidates: recovered,
            orphan_candidates: orphan,
            fragment_candidates: fragment,
            complete_candidates: complete,
            partial_candidates: partial,
            unsupported_candidates: unsupported,
            blob_count,
            blob_bytes,
        })
    }

    pub fn replay_candidates(&self) -> Result<Vec<ReplayCandidate>, JobError> {
        let mut statement = self.connection.prepare(
            "SELECT item_key, provenance, source_node_id, recovery_index, occurrence, metadata_json, recovery_unit_json \
             FROM candidates \
             WHERE status IN ('spooled', 'written', 'unsupported') ORDER BY rowid",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(
                    item_key,
                    provenance,
                    source_node_id,
                    recovery_index,
                    occurrence,
                    metadata,
                    unit,
                )| {
                    let provenance = match provenance.as_str() {
                        "normal" => CatalogProvenance::Normal,
                        "recovered" => CatalogProvenance::Recovered,
                        "orphan" => CatalogProvenance::Orphan,
                        "fragment" => CatalogProvenance::Fragment,
                        other => {
                            return Err(JobError::Integrity(format!(
                                "invalid candidate provenance {other:?}"
                            )));
                        }
                    };
                    Ok(ReplayCandidate {
                        item_key,
                        id: source_node_id
                            .map(u32::try_from)
                            .transpose()
                            .map_err(|_| JobError::Integrity("invalid source node id".to_owned()))?
                            .unwrap_or(0),
                        provenance,
                        recovery_index: recovery_index.map(u64::try_from).transpose().map_err(
                            |_| JobError::Integrity("invalid recovery index".to_owned()),
                        )?,
                        occurrence: u32::try_from(occurrence)
                            .map_err(|_| JobError::Integrity("invalid occurrence".to_owned()))?,
                        metadata: serde_json::from_str(&metadata)?,
                        unit: unit.map(|value| serde_json::from_str(&value)).transpose()?,
                    })
                },
            )
            .collect()
    }

    pub fn spooled_folders(&self) -> Result<Vec<SpooledFolder>, JobError> {
        let mut statement = self.connection.prepare(
            "SELECT source_id, parent_source_id, name, address_json, container_class \
                 FROM folders ORDER BY folder_key",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(source_id, parent_source_id, name, address, container_class)| {
                    let source_id = checked_u32(source_id, "folder source id")?;
                    let parent_source_id = parent_source_id
                        .map(|value| checked_u32(value, "folder parent source id"))
                        .transpose()?;
                    Ok(SpooledFolder {
                        address: address
                            .map(|value| serde_json::from_str(&value))
                            .transpose()?,
                        source_id,
                        parent_source_id,
                        name,
                        container_class,
                    })
                },
            )
            .collect()
    }

    pub fn spooled_candidates(&self) -> Result<Vec<SpooledCandidate>, JobError> {
        self.spooled_candidates_expected(None)
    }

    pub fn spooled_candidates_interruptible(
        &self,
        interrupted: &AtomicBool,
    ) -> Result<Vec<SpooledCandidate>, JobError> {
        self.spooled_candidates_expected(Some(interrupted))
    }

    fn spooled_candidates_expected(
        &self,
        interrupted: Option<&AtomicBool>,
    ) -> Result<Vec<SpooledCandidate>, JobError> {
        let mut statement = self.connection.prepare(
            "SELECT item_key, provenance, source_node_id, recovery_index, occurrence, \
                    completeness, metadata_json, parent_item_key, parent_attachment_index, \
                    recovery_unit_json \
             FROM candidates WHERE status = 'spooled' \
             ORDER BY provenance, source_node_id, recovery_index, occurrence, item_key",
        )?;
        let mut query = statement.query([])?;
        let mut rows = Vec::new();
        while let Some(row) = query.next()? {
            check_job_interrupted(interrupted)?;
            rows.push((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ));
        }
        drop(query);
        drop(statement);
        let mut events_by_candidate = self.spooled_events_by_candidate(interrupted)?;
        let mut candidates = Vec::with_capacity(rows.len());
        for (
            item_key,
            provenance,
            source_node_id,
            recovery_index,
            occurrence,
            completeness,
            metadata,
            parent_item_key,
            parent_attachment_index,
            unit,
        ) in rows
        {
            check_job_interrupted(interrupted)?;
            let provenance = parse_provenance(&provenance)?;
            let source_node_id = source_node_id
                .map(|value| checked_u32(value, "candidate source node id"))
                .transpose()?;
            let recovery_index = recovery_index
                .map(|value| checked_u64(value, "candidate recovery index"))
                .transpose()?;
            let occurrence = checked_u32(occurrence, "candidate occurrence")?;
            let metadata = serde_json::from_str(&metadata)?;
            let parent_attachment_index = parent_attachment_index
                .map(|value| checked_u32(value, "parent attachment index"))
                .transpose()?;
            let unit = unit.map(|value| serde_json::from_str(&value)).transpose()?;
            let events = events_by_candidate.remove(&item_key).unwrap_or_default();
            candidates.push(SpooledCandidate {
                item_key,
                provenance,
                source_node_id,
                recovery_index,
                occurrence,
                parent_item_key,
                parent_attachment_index,
                unit,
                completeness,
                metadata,
                events,
            });
        }
        if !events_by_candidate.is_empty() {
            return Err(JobError::Integrity(
                "spooled events refer to an unclaimed candidate".to_owned(),
            ));
        }
        Ok(candidates)
    }

    pub fn spooled_top_level_item_keys_interruptible(
        &self,
        interrupted: &AtomicBool,
    ) -> Result<Vec<String>, JobError> {
        let mut statement = self.connection.prepare(
            "SELECT item_key FROM candidates \
             WHERE status = 'spooled' AND parent_item_key IS NULL \
             ORDER BY provenance, source_node_id, recovery_index, occurrence, item_key",
        )?;
        let mut query = statement.query([])?;
        let mut keys = Vec::new();
        while let Some(row) = query.next()? {
            check_job_interrupted(Some(interrupted))?;
            keys.push(row.get(0)?);
        }
        Ok(keys)
    }

    pub fn spooled_top_level_candidate_headers_page_interruptible(
        &self,
        after_rowid: i64,
        page_size: u32,
        interrupted: &AtomicBool,
    ) -> Result<SpooledCandidateHeaderPage, JobError> {
        if after_rowid < 0 || page_size == 0 {
            return Err(JobError::Integrity(
                "invalid top-level candidate header page request".to_owned(),
            ));
        }
        let mut statement = self.connection.prepare(
            "SELECT rowid, item_key, provenance, source_node_id, recovery_index, occurrence, \
                    completeness, metadata_json, recovery_unit_json, embedded_path_json \
             FROM candidates \
             WHERE rowid > ?1 AND status = 'spooled' AND parent_item_key IS NULL \
             ORDER BY rowid LIMIT ?2",
        )?;
        let mut query = statement.query(params![after_rowid, i64::from(page_size)])?;
        let mut candidates = Vec::new();
        let mut ownerships = Vec::new();
        let mut next_rowid = after_rowid;
        while let Some(row) = query.next()? {
            check_job_interrupted(Some(interrupted))?;
            next_rowid = row.get(0)?;
            let item_key: String = row.get(1)?;
            let source_node_id = row
                .get::<_, Option<i64>>(3)?
                .map(|value| checked_u32(value, "candidate source node id"))
                .transpose()?;
            let metadata: serde_json::Value = serde_json::from_str(&row.get::<_, String>(7)?)?;
            let embedded_path: Vec<u32> = serde_json::from_str(&row.get::<_, String>(9)?)?;
            ownerships.push(CandidateOwnership {
                item_key: item_key.clone(),
                source_node_id,
                writable: true,
                parent_item_key: None,
                parent_attachment_index: None,
                embedded_path,
                metadata: metadata.clone(),
            });
            candidates.push(SpooledCandidate {
                item_key,
                provenance: parse_provenance(&row.get::<_, String>(2)?)?,
                source_node_id,
                recovery_index: row
                    .get::<_, Option<i64>>(4)?
                    .map(|value| checked_u64(value, "candidate recovery index"))
                    .transpose()?,
                occurrence: checked_u32(row.get(5)?, "candidate occurrence")?,
                parent_item_key: None,
                parent_attachment_index: None,
                unit: row
                    .get::<_, Option<String>>(8)?
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?,
                completeness: row.get(6)?,
                metadata,
                events: Vec::new(),
            });
        }
        Ok(SpooledCandidateHeaderPage {
            next_rowid,
            tree: SpooledCandidateTree {
                candidates,
                ownerships,
            },
        })
    }

    pub fn candidate_named_property_identities_interruptible(
        &self,
        interrupted: &AtomicBool,
    ) -> Result<Vec<libpff_sys::NamedPropertyIdentity>, JobError> {
        let mut statement = self.connection.prepare(
            "SELECT DISTINCT json_extract(e.metadata_json, '$.named_property') \
             FROM candidate_events e JOIN candidates c ON c.item_key = e.item_key \
             WHERE c.status IN ('spooled', 'written', 'unsupported', 'failed') \
               AND e.kind IN ('property', 'property_incomplete') \
               AND json_type(e.metadata_json, '$.named_property') = 'object' \
             ORDER BY 1",
        )?;
        let mut query = statement.query([])?;
        let mut identities = Vec::new();
        while let Some(row) = query.next()? {
            check_job_interrupted(Some(interrupted))?;
            identities.push(serde_json::from_str(&row.get::<_, String>(0)?)?);
        }
        Ok(identities)
    }

    pub fn spooled_candidate_tree_interruptible(
        &self,
        root_item_key: &str,
        interrupted: &AtomicBool,
    ) -> Result<SpooledCandidateTree, JobError> {
        check_job_interrupted(Some(interrupted))?;
        let tree_sql = "WITH RECURSIVE tree(item_key) AS (\
                SELECT item_key FROM candidates \
                WHERE item_key = ?1 AND status = 'spooled' \
                UNION \
                SELECT child.item_key FROM candidates child \
                JOIN tree parent ON child.parent_item_key = parent.item_key \
                WHERE child.status IN ('spooled', 'written', 'unsupported')\
            ) ";
        let candidate_sql = format!(
            "{tree_sql}\
             SELECT c.item_key, c.provenance, c.source_node_id, c.recovery_index, c.occurrence, \
                    c.completeness, c.metadata_json, c.parent_item_key, \
                    c.parent_attachment_index, c.recovery_unit_json \
             FROM tree t CROSS JOIN candidates c ON c.item_key = t.item_key \
             WHERE c.status = 'spooled' ORDER BY c.item_key"
        );
        let mut statement = self.connection.prepare(&candidate_sql)?;
        let mut query = statement.query([root_item_key])?;
        let mut rows = Vec::new();
        while let Some(row) = query.next()? {
            check_job_interrupted(Some(interrupted))?;
            rows.push((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ));
        }
        drop(query);
        drop(statement);
        if rows.is_empty() {
            return Err(JobError::Integrity(
                "requested top-level spooled candidate does not exist".to_owned(),
            ));
        }

        let event_sql = format!(
            "{tree_sql}\
             SELECT e.item_key, e.sequence, e.kind, e.metadata_json, \
                    e.blob_sha256, e.byte_len, b.pack_offset \
             FROM tree t \
             CROSS JOIN candidate_events e ON e.item_key = t.item_key \
             JOIN candidates c ON c.item_key = e.item_key \
             LEFT JOIN blobs b ON b.sha256 = e.blob_sha256 \
             WHERE c.status = 'spooled' ORDER BY e.item_key, e.sequence"
        );
        let mut statement = self.connection.prepare(&event_sql)?;
        let mut query = statement.query([root_item_key])?;
        let mut events_by_candidate = HashMap::<String, Vec<SpooledEvent>>::new();
        while let Some(row) = query.next()? {
            check_job_interrupted(Some(interrupted))?;
            let item_key = row.get::<_, String>(0)?;
            let sha256 = row.get::<_, Option<String>>(4)?;
            let byte_len = row.get::<_, Option<i64>>(5)?;
            let pack_offset = row.get::<_, Option<i64>>(6)?;
            let blob = match (sha256, byte_len) {
                (Some(sha256), Some(byte_len)) => Some(SpooledBlob {
                    sha256,
                    byte_len: checked_u64(byte_len, "candidate event byte length")?,
                    pack_offset: pack_offset
                        .map(|value| checked_u64(value, "payload pack offset"))
                        .transpose()?,
                }),
                (None, None) if pack_offset.is_none() => None,
                _ => {
                    return Err(JobError::Integrity(
                        "candidate event has an incomplete blob reference".to_owned(),
                    ));
                }
            };
            events_by_candidate
                .entry(item_key)
                .or_default()
                .push(SpooledEvent {
                    sequence: checked_u64(row.get::<_, i64>(1)?, "candidate event sequence")?,
                    kind: row.get(2)?,
                    metadata: serde_json::from_str(&row.get::<_, String>(3)?)?,
                    blob,
                });
        }
        drop(query);
        drop(statement);

        let mut candidates = Vec::with_capacity(rows.len());
        for (
            item_key,
            provenance,
            source_node_id,
            recovery_index,
            occurrence,
            completeness,
            metadata,
            parent_item_key,
            parent_attachment_index,
            unit,
        ) in rows
        {
            check_job_interrupted(Some(interrupted))?;
            candidates.push(SpooledCandidate {
                events: events_by_candidate.remove(&item_key).unwrap_or_default(),
                item_key,
                provenance: parse_provenance(&provenance)?,
                source_node_id: source_node_id
                    .map(|value| checked_u32(value, "candidate source node id"))
                    .transpose()?,
                recovery_index: recovery_index
                    .map(|value| checked_u64(value, "candidate recovery index"))
                    .transpose()?,
                occurrence: checked_u32(occurrence, "candidate occurrence")?,
                parent_item_key,
                parent_attachment_index: parent_attachment_index
                    .map(|value| checked_u32(value, "parent attachment index"))
                    .transpose()?,
                unit: unit.map(|value| serde_json::from_str(&value)).transpose()?,
                completeness,
                metadata: serde_json::from_str(&metadata)?,
            });
        }
        if !events_by_candidate.is_empty() {
            return Err(JobError::Integrity(
                "candidate tree events refer to an unclaimed candidate".to_owned(),
            ));
        }

        let ownership_sql = format!(
            "{tree_sql}\
             SELECT c.item_key, c.source_node_id, c.status, c.parent_item_key, \
                    c.parent_attachment_index, c.embedded_path_json, c.metadata_json \
             FROM tree t CROSS JOIN candidates c ON c.item_key = t.item_key \
             ORDER BY c.item_key"
        );
        let mut statement = self.connection.prepare(&ownership_sql)?;
        let mut query = statement.query([root_item_key])?;
        let mut ownerships = Vec::new();
        while let Some(row) = query.next()? {
            check_job_interrupted(Some(interrupted))?;
            let parent_item_key = row.get::<_, Option<String>>(3)?;
            let parent_attachment_index = row.get::<_, Option<i64>>(4)?;
            if parent_item_key.is_some() != parent_attachment_index.is_some() {
                return Err(JobError::Integrity(
                    "candidate has incomplete embedded ownership".to_owned(),
                ));
            }
            ownerships.push(CandidateOwnership {
                item_key: row.get(0)?,
                source_node_id: row
                    .get::<_, Option<i64>>(1)?
                    .map(|value| checked_u32(value, "candidate source node id"))
                    .transpose()?,
                writable: row.get::<_, String>(2)? == "spooled",
                parent_item_key,
                parent_attachment_index: parent_attachment_index
                    .map(|value| checked_u32(value, "parent attachment index"))
                    .transpose()?,
                embedded_path: serde_json::from_str(&row.get::<_, String>(5)?)?,
                metadata: serde_json::from_str(&row.get::<_, String>(6)?)?,
            });
        }
        Ok(SpooledCandidateTree {
            candidates,
            ownerships,
        })
    }

    pub fn candidate_ownerships(&self) -> Result<Vec<CandidateOwnership>, JobError> {
        self.candidate_ownerships_expected(None)
    }

    pub fn candidate_ownerships_interruptible(
        &self,
        interrupted: &AtomicBool,
    ) -> Result<Vec<CandidateOwnership>, JobError> {
        self.candidate_ownerships_expected(Some(interrupted))
    }

    fn candidate_ownerships_expected(
        &self,
        interrupted: Option<&AtomicBool>,
    ) -> Result<Vec<CandidateOwnership>, JobError> {
        let mut statement = self.connection.prepare(
            "SELECT item_key, source_node_id, status, parent_item_key, \
                    parent_attachment_index, embedded_path_json, metadata_json \
             FROM candidates WHERE status IN ('spooled', 'written', 'unsupported') ORDER BY item_key",
        )?;
        let mut query = statement.query([])?;
        let mut rows = Vec::new();
        while let Some(row) = query.next()? {
            check_job_interrupted(interrupted)?;
            rows.push((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ));
        }
        rows.into_iter()
            .map(
                |(
                    item_key,
                    source_node_id,
                    status,
                    parent_item_key,
                    parent_attachment_index,
                    embedded_path,
                    metadata,
                )| {
                    if parent_item_key.is_some() != parent_attachment_index.is_some() {
                        return Err(JobError::Integrity(
                            "candidate has incomplete embedded ownership".to_owned(),
                        ));
                    }
                    Ok(CandidateOwnership {
                        item_key,
                        source_node_id: source_node_id
                            .map(|value| checked_u32(value, "candidate source node id"))
                            .transpose()?,
                        writable: status == "spooled",
                        parent_item_key,
                        parent_attachment_index: parent_attachment_index
                            .map(|value| checked_u32(value, "parent attachment index"))
                            .transpose()?,
                        embedded_path: serde_json::from_str(&embedded_path)?,
                        metadata: serde_json::from_str(&metadata)?,
                    })
                },
            )
            .collect()
    }

    pub fn open_blob(&self, blob: &SpooledBlob) -> Result<File, JobError> {
        self.open_blob_expected(blob, None)
    }

    pub fn open_blob_interruptible(
        &self,
        blob: &SpooledBlob,
        interrupted: &AtomicBool,
    ) -> Result<File, JobError> {
        self.open_blob_expected(blob, Some(interrupted))
    }

    fn open_blob_expected(
        &self,
        blob: &SpooledBlob,
        interrupted: Option<&AtomicBool>,
    ) -> Result<File, JobError> {
        check_job_interrupted(interrupted)?;
        if let Some(offset) = blob.pack_offset {
            if !valid_blob_hash(&blob.sha256) {
                return Err(JobError::Integrity(
                    "invalid payload pack digest".to_owned(),
                ));
            }
            let end = offset
                .checked_add(blob.byte_len)
                .ok_or_else(|| JobError::Integrity("payload pack range overflow".to_owned()))?;
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&self.payload_pack_path)
                .map_err(|source| io_error(&self.payload_pack_path, source))?;
            if file
                .metadata()
                .map_err(|source| io_error(&self.payload_pack_path, source))?
                .len()
                < end
            {
                return Err(JobError::Integrity(
                    "payload pack range exceeds the durable file".to_owned(),
                ));
            }
            file.seek(std::io::SeekFrom::Start(offset))
                .map_err(|source| io_error(&self.payload_pack_path, source))?;
            return Ok(file);
        }
        let (expected, inline) = self.blob_record(blob)?;
        if expected != blob.byte_len {
            return Err(JobError::Integrity(format!(
                "spool blob {} length disagrees with the ledger",
                blob.sha256
            )));
        }
        if let Some(data) = inline {
            let file_path = self.spool.join(&blob.sha256);
            if file_path
                .try_exists()
                .map_err(|source| io_error(&file_path, source))?
            {
                return Err(JobError::Integrity(format!(
                    "blob {} has multiple storage representations",
                    blob.sha256
                )));
            }
            verify_blob_bytes(&blob.sha256, blob.byte_len, &data)?;
            let owned =
                rustix::fs::memfd_create("pstforge-inline-blob", rustix::fs::MemfdFlags::CLOEXEC)
                    .map_err(|source| io_error(&self.partial, source.into()))?;
            let mut file = File::from(owned);
            file.write_all(&data)
                .map_err(|source| io_error(&self.partial, source))?;
            file.seek(std::io::SeekFrom::Start(0))
                .map_err(|source| io_error(&self.partial, source))?;
            check_job_interrupted(interrupted)?;
            return Ok(file);
        }
        open_verified_blob_with_interrupt(
            &self.spool.join(&blob.sha256),
            &blob.sha256,
            blob.byte_len,
            interrupted,
        )
    }

    /// Return a held private path after verifying that the ledger and payload
    /// agree. Inline data is materialized only as disposable writer scratch.
    pub fn verified_blob_path(&self, blob: &SpooledBlob) -> Result<PathBuf, JobError> {
        self.verified_blob_path_expected(blob, None)
    }

    pub fn verified_blob_path_interruptible(
        &self,
        blob: &SpooledBlob,
        interrupted: &AtomicBool,
    ) -> Result<PathBuf, JobError> {
        self.verified_blob_path_expected(blob, Some(interrupted))
    }

    fn verified_blob_path_expected(
        &self,
        blob: &SpooledBlob,
        interrupted: Option<&AtomicBool>,
    ) -> Result<PathBuf, JobError> {
        check_job_interrupted(interrupted)?;
        if let Some(offset) = blob.pack_offset {
            let end = offset
                .checked_add(blob.byte_len)
                .ok_or_else(|| JobError::Integrity("payload pack range overflow".to_owned()))?;
            if !valid_blob_hash(&blob.sha256) || end > self.payload_pack_len()? {
                return Err(JobError::Integrity(
                    "payload pack blob has an invalid range".to_owned(),
                ));
            }
            return Ok(self.payload_pack_path.clone());
        }
        let (expected, inline) = self.blob_record(blob)?;
        if expected != blob.byte_len {
            return Err(JobError::Integrity(format!(
                "spool blob {} length disagrees with the ledger",
                blob.sha256
            )));
        }
        let Some(data) = inline else {
            drop(self.open_blob_expected(blob, interrupted)?);
            return Ok(self.spool.join(&blob.sha256));
        };
        verify_blob_bytes(&blob.sha256, blob.byte_len, &data)?;
        let cache = self.inline_cache_path()?;
        let destination = cache.join(format!(".tmp-{}", blob.sha256));
        match destination.symlink_metadata() {
            Ok(_) => verify_blob(&destination, &blob.sha256, blob.byte_len)?,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let mut temporary =
                    NamedTempFile::new_in(&cache).map_err(|source| io_error(&cache, source))?;
                temporary
                    .write_all(&data)
                    .map_err(|source| io_error(temporary.path(), source))?;
                temporary
                    .flush()
                    .map_err(|source| io_error(temporary.path(), source))?;
                temporary
                    .persist_noclobber(&destination)
                    .map_err(|error| io_error(&destination, error.error))?;
                verify_blob(&destination, &blob.sha256, blob.byte_len)?;
            }
            Err(source) => return Err(io_error(&destination, source)),
        }
        check_job_interrupted(interrupted)?;
        Ok(destination)
    }

    fn blob_record(&self, blob: &SpooledBlob) -> Result<(u64, Option<Vec<u8>>), JobError> {
        if !valid_blob_hash(&blob.sha256) {
            return Err(JobError::Integrity("invalid spool blob digest".to_owned()));
        }
        let (expected, inline_len, data) = self.connection.query_row(
            "SELECT b.byte_len, length(i.data), \
                    CASE WHEN length(i.data) <= ?2 THEN i.data END \
             FROM blobs b LEFT JOIN inline_blobs i ON i.sha256 = b.sha256 \
             WHERE b.sha256 = ?1",
            params![&blob.sha256, INLINE_BLOB_MAX_BYTES],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Option<u64>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            },
        )?;
        match (inline_len, data) {
            (None, None) => Ok((expected, None)),
            (Some(length), Some(data))
                if length == expected
                    && length <= INLINE_BLOB_MAX_BYTES
                    && u64::try_from(data.len()).ok() == Some(length) =>
            {
                Ok((expected, Some(data)))
            }
            _ => Err(JobError::Integrity(format!(
                "inline blob {} has an invalid bounded representation",
                blob.sha256
            ))),
        }
    }

    fn inline_cache_path(&self) -> Result<PathBuf, JobError> {
        let mut cache = self.inline_cache_directory.borrow_mut();
        if cache.is_none() {
            let path = self.partial.join(INLINE_CACHE_DIRECTORY);
            match fs::create_dir(&path) {
                Ok(()) => set_mode(&path, 0o700)?,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(source) => return Err(io_error(&path, source)),
            }
            let directory = open_directory(&path)?;
            validate_private_directory(&directory, &path)?;
            *cache = Some(directory);
        }
        let directory = cache
            .as_ref()
            .ok_or_else(|| JobError::Integrity("inline cache handle is missing".to_owned()))?;
        Ok(fd_path(directory.as_raw_fd()))
    }

    pub fn staged_part_path(&self, filename: &str) -> Result<PathBuf, JobError> {
        if !valid_leaf_name(filename) || !filename.ends_with(".partial") {
            return Err(JobError::UnsafePath(PathBuf::from(filename)));
        }
        Ok(self.partial.join(filename))
    }

    pub fn publish_recovery_log(&self, contents: &str) -> Result<(), JobError> {
        if contents.len() > MAX_RECOVERY_LOG_BYTES {
            return Err(JobError::Integrity(
                "recovery log exceeds the supported size".to_owned(),
            ));
        }
        let staged_name = Path::new("recovery.log.partial");
        let staged_path = self.partial.join(staged_name);
        match staged_path.symlink_metadata() {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    || !private_state_attributes_valid(
                        metadata.is_file(),
                        metadata.uid(),
                        metadata.mode(),
                        Some(metadata.nlink()),
                    ) =>
            {
                return Err(JobError::UnsafePath(staged_path));
            }
            Ok(_) => {
                fs::remove_file(&staged_path).map_err(|source| io_error(&staged_path, source))?;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(&staged_path, source)),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&staged_path)
            .map_err(|source| io_error(&staged_path, source))?;
        file.write_all(contents.as_bytes())
            .map_err(|source| io_error(&staged_path, source))?;
        file.sync_all()
            .map_err(|source| io_error(&staged_path, source))?;

        let final_name = Path::new("recovery.log");
        let final_path = fd_path(self._job_directory.as_raw_fd()).join(final_name);
        replace_at(
            &self._partial_directory,
            staged_name,
            &self._job_directory,
            final_name,
            &final_path,
        )?;
        sync_file(&self._job_directory, &final_path)?;
        verify_recovery_log(&final_path, contents)
    }

    pub fn publish_validated_part(
        &mut self,
        staged_filename: &str,
        part: &PublishedPart,
        sidecar: &PartSidecar,
        item_keys: &[String],
    ) -> Result<(), JobError> {
        self.publish_validated_part_with_interrupt(staged_filename, part, sidecar, item_keys, None)
    }

    pub fn publish_validated_part_interruptible(
        &mut self,
        staged_filename: &str,
        part: &PublishedPart,
        sidecar: &PartSidecar,
        item_keys: &[String],
        interrupted: &AtomicBool,
    ) -> Result<(), JobError> {
        self.publish_validated_part_with_interrupt(
            staged_filename,
            part,
            sidecar,
            item_keys,
            Some(interrupted),
        )
    }

    pub fn publish_constructed_part_interruptible(
        &mut self,
        staged_filename: &str,
        part: &PublishedPart,
        sidecar: &PartSidecar,
        item_keys: &[String],
        interrupted: &AtomicBool,
    ) -> Result<(), JobError> {
        self.publish_part_with_interrupt(
            staged_filename,
            part,
            sidecar,
            item_keys,
            Some(interrupted),
        )
    }

    fn publish_validated_part_with_interrupt(
        &mut self,
        staged_filename: &str,
        part: &PublishedPart,
        sidecar: &PartSidecar,
        item_keys: &[String],
        interrupted: Option<&AtomicBool>,
    ) -> Result<(), JobError> {
        self.publish_part_with_interrupt(staged_filename, part, sidecar, item_keys, interrupted)
    }

    fn publish_part_with_interrupt(
        &mut self,
        staged_filename: &str,
        part: &PublishedPart,
        sidecar: &PartSidecar,
        item_keys: &[String],
        interrupted: Option<&AtomicBool>,
    ) -> Result<(), JobError> {
        self.commit_candidate_batch()?;
        validate_part_record(part)?;
        validate_sidecar(part, sidecar)?;
        let staged = self.staged_part_path(staged_filename)?;
        verify_part_artifact(&staged, part, sidecar, interrupted)?;
        check_interrupted(interrupted)?;
        let final_path = self.parts.join(&part.filename);
        let sidecar_filename = part.filename.trim_end_matches(".pst").to_owned() + ".json";
        let sidecar_path = self.manifests.join(&sidecar_filename);
        let staged_sidecar_filename = sidecar_filename.clone() + ".partial";
        let staged_sidecar_path = self.partial.join(&staged_sidecar_filename);
        refuse_existing(&final_path)?;
        refuse_existing(&sidecar_path)?;
        refuse_existing(&staged_sidecar_path)?;
        write_sidecar_partial(&staged_sidecar_path, sidecar)?;

        check_interrupted(interrupted)?;
        self.record_publication_intent(part, sidecar, item_keys)?;

        rename_noclobber(
            &self._partial_directory,
            Path::new(staged_filename),
            &self._parts_directory,
            Path::new(&part.filename),
            &final_path,
        )?;
        sync_file(&self._parts_directory, &self.parts)?;
        check_interrupted(interrupted)?;
        verify_part_artifact(&final_path, part, sidecar, interrupted)?;
        check_interrupted(interrupted)?;
        rename_noclobber(
            &self._partial_directory,
            Path::new(&staged_sidecar_filename),
            &self._manifests_directory,
            Path::new(&sidecar_filename),
            &sidecar_path,
        )?;
        sync_file(&self._manifests_directory, &self.manifests)?;
        check_interrupted(interrupted)?;
        verify_sidecar_artifact(&sidecar_path, sidecar)?;
        check_interrupted(interrupted)?;
        commit_published_part_transaction(&mut self.connection, part, sidecar, item_keys, true)
    }

    fn record_publication_intent(
        &self,
        part: &PublishedPart,
        sidecar: &PartSidecar,
        item_keys: &[String],
    ) -> Result<(), JobError> {
        if self.active.is_some() || item_keys.is_empty() {
            return Err(JobError::EventSequence(
                "cannot publish an empty part during an active candidate".to_owned(),
            ));
        }
        for item_key in item_keys {
            let available = self.connection.query_row(
                "SELECT status = 'spooled' FROM candidates WHERE item_key = ?1",
                [item_key],
                |row| row.get::<_, bool>(0),
            )?;
            if !available {
                return Err(JobError::Integrity(format!(
                    "candidate {item_key} is not available for part assignment"
                )));
            }
        }
        self.connection.execute(
            "INSERT INTO publication_intents(part_index, part_json, sidecar_json, item_keys_json) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                part.index,
                serde_json::to_string(part)?,
                serde_json::to_string(sidecar)?,
                serde_json::to_string(item_keys)?,
            ],
        )?;
        Ok(())
    }

    pub fn commit_published_part(
        &mut self,
        part: &PublishedPart,
        sidecar: &PartSidecar,
        item_keys: &[String],
    ) -> Result<(), JobError> {
        if self.active.is_some() || item_keys.is_empty() {
            return Err(JobError::EventSequence(
                "cannot commit an empty part during an active candidate".to_owned(),
            ));
        }
        self.commit_candidate_batch()?;
        validate_part_record(part)?;
        validate_sidecar(part, sidecar)?;
        verify_part_artifact(&self.parts.join(&part.filename), part, sidecar, None)?;
        commit_published_part_transaction(&mut self.connection, part, sidecar, item_keys, false)
    }

    fn spooled_events_by_candidate(
        &self,
        interrupted: Option<&AtomicBool>,
    ) -> Result<HashMap<String, Vec<SpooledEvent>>, JobError> {
        let mut statement = self.connection.prepare(
            "SELECT e.item_key, e.sequence, e.kind, e.metadata_json, \
                    e.blob_sha256, e.byte_len, \
                    b.pack_offset \
             FROM candidate_events e \
             JOIN candidates c ON c.item_key = e.item_key \
             LEFT JOIN blobs b ON b.sha256 = e.blob_sha256 \
             WHERE c.status = 'spooled' ORDER BY e.item_key, e.sequence",
        )?;
        let mut query = statement.query([])?;
        let mut events = HashMap::<String, Vec<SpooledEvent>>::new();
        while let Some(row) = query.next()? {
            check_job_interrupted(interrupted)?;
            let item_key = row.get::<_, String>(0)?;
            let sha256 = row.get::<_, Option<String>>(4)?;
            let byte_len = row.get::<_, Option<i64>>(5)?;
            let pack_offset = row.get::<_, Option<i64>>(6)?;
            let blob = match (sha256, byte_len) {
                (Some(sha256), Some(byte_len)) => Some(SpooledBlob {
                    sha256,
                    byte_len: checked_u64(byte_len, "candidate event byte length")?,
                    pack_offset: pack_offset
                        .map(|value| checked_u64(value, "payload pack offset"))
                        .transpose()?,
                }),
                (None, None) if pack_offset.is_none() => None,
                _ => {
                    return Err(JobError::Integrity(
                        "candidate event has an incomplete blob reference".to_owned(),
                    ));
                }
            };
            events.entry(item_key).or_default().push(SpooledEvent {
                sequence: checked_u64(row.get::<_, i64>(1)?, "candidate event sequence")?,
                kind: row.get(2)?,
                metadata: serde_json::from_str(&row.get::<_, String>(3)?)?,
                blob,
            });
        }
        Ok(events)
    }

    fn start_candidate(&mut self, start: CandidateStart) -> Result<(), JobError> {
        let CandidateStart {
            mut metadata,
            id,
            provenance,
            recovery_index,
            parent_message_id,
            parent_attachment_index,
            embedded_path,
            supported,
        } = start;
        if self.pending_named_property.is_some() {
            return Err(JobError::EventSequence(
                "message started after an unmatched named property identity".to_owned(),
            ));
        }
        if self
            .property
            .as_ref()
            .is_some_and(|property| matches!(property.descriptor.owner, PropertyOwner::Folder(_)))
        {
            self.property = None;
        }
        if self.property.is_some() {
            return Err(JobError::EventSequence(
                "message started during a non-folder property".to_owned(),
            ));
        }
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "message started before the previous message ended".to_owned(),
            ));
        }
        let provenance = match provenance {
            CatalogProvenance::Normal => "normal",
            CatalogProvenance::Recovered => "recovered",
            CatalogProvenance::Orphan => "orphan",
            CatalogProvenance::Fragment => "fragment",
        };
        let source_node_id = (id != 0).then_some(i64::from(id));
        let occurrence = self.connection.query_row(
            "SELECT COUNT(*) FROM candidates \
             WHERE provenance = ?1 AND source_node_id IS ?2 AND recovery_index IS ?3",
            params![provenance, source_node_id, recovery_index],
            |row| row.get::<_, u32>(0),
        )?;
        let key = format!(
            "{provenance}:{}:{}:{occurrence}",
            source_node_id.map_or_else(|| "-".to_owned(), |value| value.to_string()),
            recovery_index.map_or_else(|| "-".to_owned(), |value| value.to_string())
        );
        let parent_item_key = parent_message_id
            .map(|_| {
                let mut parent_path = embedded_path.clone();
                let attachment_index = parent_path.pop().ok_or_else(|| {
                    JobError::EventSequence("embedded message path is empty".to_owned())
                })?;
                if Some(attachment_index) != parent_attachment_index {
                    return Err(JobError::EventSequence(
                        "embedded message path disagrees with its attachment".to_owned(),
                    ));
                }
                if let Some(key) = self.recent_candidates.get(&parent_path) {
                    return Ok(key.clone());
                }
                self.connection
                    .query_row(
                        "SELECT item_key FROM candidates \
                         WHERE provenance = ?1 AND recovery_index IS ?2 \
                           AND embedded_path_json = ?3 AND status != 'pending' \
                         ORDER BY rowid DESC LIMIT 1",
                        params![
                            provenance,
                            recovery_index,
                            serde_json::to_string(&parent_path)?
                        ],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(|error| match error {
                        rusqlite::Error::QueryReturnedNoRows => JobError::EventSequence(
                            "embedded message parent is not durably committed".to_owned(),
                        ),
                        other => other.into(),
                    })
            })
            .transpose()?;
        if let Some(parent_id) = parent_message_id {
            if let Some(durable_parent_id) = self.replayed_source_ids.get(&parent_id) {
                metadata["parent_message_id"] = serde_json::json!(durable_parent_id);
            }
        }
        if parent_item_key.is_some() != parent_attachment_index.is_some() {
            return Err(JobError::EventSequence(
                "embedded message parent attachment is incomplete".to_owned(),
            ));
        }
        let metadata_json = serde_json::to_string(&metadata)?;
        let unit_json = self
            .unit
            .map(|unit| serde_json::to_string(&unit))
            .transpose()?;
        let embedded_path_json = serde_json::to_string(&embedded_path)?;
        let pack_start = self.payload_pack_position.get();
        self.begin_candidate()?;
        let result = self.connection.execute(
            "INSERT INTO candidates(\
                item_key, provenance, source_node_id, recovery_index, occurrence,\
                completeness, status, metadata_json, recovery_unit_json,\
                parent_item_key, parent_attachment_index, embedded_path_json\
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'damaged', 'pending', ?6, ?7, ?8, ?9, ?10)",
            params![
                key,
                provenance,
                source_node_id,
                recovery_index,
                occurrence,
                metadata_json,
                unit_json,
                parent_item_key,
                parent_attachment_index,
                embedded_path_json,
            ],
        );
        if let Err(error) = result {
            let _ = self.abort_candidate();
            return Err(error.into());
        }
        self.active = Some(ActiveCandidate {
            key,
            message_id: id,
            pack_start,
            sequence: 0,
            recipients: HashSet::new(),
            supported,
            embedded_path,
        });
        Ok(())
    }

    fn record_event(
        &mut self,
        kind: &str,
        metadata: serde_json::Value,
        blob: Option<BlobRef>,
    ) -> Result<(), JobError> {
        let active = self
            .active
            .as_mut()
            .ok_or_else(|| JobError::EventSequence(format!("{kind} event outside a message")))?;
        active.sequence = active
            .sequence
            .checked_add(1)
            .ok_or_else(|| JobError::EventSequence("candidate event count overflow".to_owned()))?;
        self.connection
            .prepare_cached(
                "INSERT INTO candidate_events(\
                    item_key, sequence, kind, metadata_json, blob_sha256, byte_len\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                active.key,
                active.sequence,
                kind,
                serde_json::to_string(&metadata)?,
                blob.as_ref().map(|value| value.sha256.as_str()),
                blob.as_ref().map(|value| value.bytes),
            ])?;
        Ok(())
    }

    fn finish_property(&mut self, descriptor: PropertyDescriptor) -> Result<(), JobError> {
        let active = self.property.take().ok_or_else(|| {
            JobError::EventSequence("property ended without a matching start".to_owned())
        })?;
        if active.descriptor != descriptor {
            return Err(JobError::EventSequence(
                "property end does not match property start".to_owned(),
            ));
        }
        if active.record {
            let captured_size = self.captured_property_bytes(descriptor.data_size);
            let blob = self.finish_blob(active.blob, Some(captured_size))?;
            let stream_type = descriptor
                .value_type
                .is_some_and(|value| matches!(value, 0x001E | 0x001F | 0x0102));
            let direct_id = (matches!(self.capture_mode, CatalogCaptureMode::Bounded { .. })
                && (captured_size < descriptor.data_size
                    || (active.named_property.is_none() && stream_type)))
                .then(|| self.take_direct_blob_id())
                .transpose()?;
            let mut metadata = property_json(descriptor, active.named_property.as_ref());
            if let Some(direct_id) = direct_id {
                metadata["direct_id"] = json!(direct_id);
            }
            self.record_event(
                if direct_id.is_some() {
                    "property_direct"
                } else {
                    "property"
                },
                metadata,
                Some(blob),
            )
        } else {
            self.discard_blob(active.blob, descriptor.data_size)
        }
    }

    fn finish_attachment(&mut self, complete: bool) -> Result<(), JobError> {
        let Some(active) = self.attachment.take() else {
            return Ok(());
        };
        if let Some(blob) = active.blob {
            let actual = blob.bytes;
            let captured_expected = active.expected.map(|expected| {
                if complete {
                    self.captured_attachment_bytes(expected)
                } else {
                    actual
                }
            });
            let blob = self.finish_blob(blob, captured_expected)?;
            let direct_id = if matches!(self.capture_mode, CatalogCaptureMode::Bounded { .. })
                && complete
                && active.expected != Some(0)
            {
                Some(self.take_direct_blob_id()?)
            } else {
                None
            };
            self.record_event(
                if direct_id.is_some() {
                    "attachment_direct"
                } else if complete {
                    "attachment_data"
                } else {
                    "attachment_partial"
                },
                json!({
                    "message_id": active.message_id,
                    "index": active.index,
                    "declared_size": active.expected,
                    "actual_size": actual,
                    "direct_id": direct_id,
                }),
                Some(blob),
            )?;
        } else if complete && active.attachment_type == Some(i32::from(b'i')) {
            return Ok(());
        } else if complete && active.attachment_type == Some(i32::from(b'r')) {
            self.record_event(
                "attachment_reference",
                json!({
                    "message_id": active.message_id,
                    "index": active.index,
                    "declared_size": active.expected,
                }),
                None,
            )?;
        } else if complete && active.expected == Some(0) {
            let blob = self.new_blob_writer()?;
            let blob = self.finish_blob(blob, Some(0))?;
            self.record_event(
                "attachment_data",
                json!({
                    "message_id": active.message_id,
                    "index": active.index,
                    "declared_size": active.expected,
                    "actual_size": 0,
                }),
                Some(blob),
            )?;
        } else if complete {
            return Err(JobError::EventSequence(
                "attachment declared data but emitted no bytes".to_owned(),
            ));
        } else if !complete {
            self.record_event(
                "attachment_missing",
                json!({
                    "message_id": active.message_id,
                    "index": active.index,
                    "declared_size": active.expected,
                }),
                None,
            )?;
        }
        Ok(())
    }

    fn finish_blob(
        &mut self,
        blob: BlobWriter,
        expected: Option<u64>,
    ) -> Result<BlobRef, JobError> {
        if let Some(expected) = expected {
            if expected != blob.bytes {
                return Err(JobError::BlobLength {
                    expected,
                    actual: blob.bytes,
                });
            }
        }
        let sha256 = digest_hex(blob.hasher.clone().finalize().as_slice());
        if let Some(data) = blob.inline {
            verify_blob_bytes(&sha256, blob.bytes, &data)?;
            let stored = self
                .connection
                .prepare_cached("SELECT byte_len FROM blobs WHERE sha256 = ?1")?
                .query_row([&sha256], |row| row.get::<_, u64>(0))
                .optional()?;
            if let Some(stored_len) = stored {
                if stored_len != blob.bytes {
                    return Err(JobError::Integrity(format!(
                        "blob {sha256} length disagrees with the ledger"
                    )));
                }
                let stored_data = self.connection.query_row(
                    "SELECT data FROM inline_blobs WHERE sha256 = ?1",
                    [&sha256],
                    |row| row.get::<_, Vec<u8>>(0),
                )?;
                verify_blob_bytes(&sha256, blob.bytes, &stored_data)?;
            } else {
                self.connection
                    .prepare_cached(
                        "INSERT INTO blobs(sha256, byte_len, pack_offset) VALUES (?1, ?2, NULL)",
                    )?
                    .execute(params![&sha256, blob.bytes])?;
                self.connection
                    .prepare_cached("INSERT INTO inline_blobs(sha256, data) VALUES (?1, ?2)")?
                    .execute(params![&sha256, data])?;
            }
            return Ok(BlobRef {
                sha256,
                bytes: blob.bytes,
            });
        }
        let expected_end = blob
            .start_offset
            .checked_add(blob.bytes)
            .ok_or_else(|| JobError::Integrity("payload pack offset overflow".to_owned()))?;
        if self.payload_pack_position.get() != expected_end {
            return Err(JobError::Integrity(
                "payload pack changed during an active blob".to_owned(),
            ));
        }
        let stored = self
            .connection
            .prepare_cached(
                "SELECT b.byte_len, b.pack_offset, EXISTS(\
                    SELECT 1 FROM inline_blobs i WHERE i.sha256 = b.sha256\
                 ) FROM blobs b WHERE b.sha256 = ?1",
            )?
            .query_row([&sha256], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Option<u64>>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })
            .optional()?;
        if let Some((stored_len, pack_offset, inline)) = stored {
            if stored_len != blob.bytes {
                return Err(JobError::Integrity(format!(
                    "blob {sha256} length disagrees with the ledger"
                )));
            }
            match (pack_offset, inline) {
                (Some(offset), false) => verify_pack_range(
                    &mut self.payload_pack,
                    &self.payload_pack_path,
                    expected_end,
                    offset,
                    stored_len,
                    &sha256,
                    None,
                )?,
                (None, _) => {
                    drop(self.open_blob_expected(
                        &SpooledBlob {
                            sha256: sha256.clone(),
                            byte_len: stored_len,
                            pack_offset: None,
                        },
                        None,
                    )?);
                }
                (Some(_), true) => {
                    return Err(JobError::Integrity(format!(
                        "blob {sha256} has multiple storage representations"
                    )));
                }
            }
            self.truncate_payload_pack(blob.start_offset)?;
        } else {
            self.connection
                .prepare_cached(
                    "INSERT INTO blobs(sha256, byte_len, pack_offset) VALUES (?1, ?2, ?3)",
                )?
                .execute(params![&sha256, blob.bytes, blob.start_offset])?;
        }
        Ok(BlobRef {
            sha256,
            bytes: blob.bytes,
        })
    }

    fn discard_blob(&mut self, blob: BlobWriter, expected: u64) -> Result<(), JobError> {
        if blob.bytes != expected {
            return Err(JobError::BlobLength {
                expected,
                actual: blob.bytes,
            });
        }
        if blob.inline.is_some() {
            Ok(())
        } else {
            self.truncate_payload_pack(blob.start_offset)
        }
    }

    fn rollback(&mut self) {
        self.property = None;
        self.pending_named_property = None;
        self.attachment = None;
        self.unit = None;
        self.recent_candidates.clear();
        self.replayed_source_ids.clear();
        self.active = None;
        if self.batch_open.get() {
            let _ = self.connection.execute_batch("ROLLBACK");
            let _ = self.truncate_payload_pack(self.batch_pack_start.get());
            self.batch_open.set(false);
            self.batch_candidates.set(0);
        }
    }

    fn captured_property_bytes(&self, declared: u64) -> u64 {
        match self.capture_mode {
            CatalogCaptureMode::Full => declared,
            CatalogCaptureMode::Bounded {
                property_prefix_bytes,
                ..
            } => declared.min(property_prefix_bytes),
        }
    }

    fn captured_attachment_bytes(&self, declared: u64) -> u64 {
        match self.capture_mode {
            CatalogCaptureMode::Full => declared,
            CatalogCaptureMode::Bounded {
                attachment_prefix_bytes,
                ..
            } => declared.min(attachment_prefix_bytes),
        }
    }

    fn take_direct_blob_id(&mut self) -> Result<u64, JobError> {
        let id = self.next_direct_blob_id;
        self.next_direct_blob_id = self
            .next_direct_blob_id
            .checked_add(1)
            .ok_or_else(|| JobError::Integrity("direct blob identifier overflow".to_owned()))?;
        Ok(id)
    }
}

pub fn read_validated_report_snapshot(
    job_directory: &Path,
) -> Result<ValidatedReportSnapshot, JobError> {
    let parent_path = job_parent(job_directory);
    let parent_directory = open_directory(parent_path)?;
    let job_name = job_directory
        .file_name()
        .ok_or_else(|| JobError::UnsafePath(job_directory.to_path_buf()))?;
    let held_job_path = fd_path(parent_directory.as_raw_fd()).join(job_name);
    let job_directory_handle = open_directory(&held_job_path)?;
    let held_job_path = fd_path(job_directory_handle.as_raw_fd());
    validate_private_directory(&job_directory_handle, &held_job_path)?;

    let private_directory = open_directory(&held_job_path.join(".pstforge"))?;
    let private_root = fd_path(private_directory.as_raw_fd());
    validate_private_directory(&private_directory, &private_root)?;
    validate_private_root_entries(&private_root)?;

    let parts_directory = open_directory(&held_job_path.join("parts"))?;
    let parts = fd_path(parts_directory.as_raw_fd());
    validate_private_directory(&parts_directory, &parts)?;
    let manifests_directory = open_directory(&private_root.join("manifests"))?;
    let manifests = fd_path(manifests_directory.as_raw_fd());
    validate_private_directory(&manifests_directory, &manifests)?;
    let spool_directory = open_directory(&private_root.join("spool"))?;
    validate_private_directory(&spool_directory, &private_root.join("spool"))?;
    let partial_directory = open_directory(&private_root.join("partial"))?;
    validate_private_directory(&partial_directory, &private_root.join("partial"))?;

    let database = private_root.join("job.sqlite3");
    validate_private_file(&database, true)?;
    let wal = private_root.join("job.sqlite3-wal");
    let shared_memory = private_root.join("job.sqlite3-shm");
    validate_private_file(&wal, false)?;
    validate_private_file(&shared_memory, false)?;
    if path_exists(&wal)? || path_exists(&shared_memory)? {
        return Err(JobError::ReportUnavailable(
            "the job ledger is active or has not completed its final checkpoint",
        ));
    }
    let database_uri = format!("file:{}?immutable=1", database.display());
    let connection = Connection::open_with_flags(
        database_uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    connection.execute_batch("PRAGMA trusted_schema = OFF; PRAGMA query_only = ON;")?;
    if read_schema_version(&connection)? != JOB_SCHEMA_VERSION {
        return Err(JobError::ReportUnavailable(
            "the job schema is not supported by this PSTForge version",
        ));
    }
    let integrity = connection.query_row("PRAGMA integrity_check(1)", [], |row| {
        row.get::<_, String>(0)
    })?;
    if integrity != "ok" {
        return Err(JobError::Integrity(integrity));
    }
    validate_foreign_keys(&connection)?;
    read_candidate_rejection_counts(&connection)?;
    validate_part_store(&connection, &parts, &manifests, None)?;

    let snapshot_length = connection
        .query_row(
            "SELECT length(value) FROM job_metadata WHERE key = 'split_report_snapshot'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .optional()?
        .ok_or(JobError::ReportUnavailable(
            "this job predates the 0.5.0 report snapshot",
        ))?;
    if snapshot_length
        > u64::try_from(MAX_REPORT_SNAPSHOT_BYTES)
            .map_err(|_| JobError::Integrity("report size limit overflow".to_owned()))?
    {
        return Err(JobError::Integrity(
            "report snapshot exceeds the supported size".to_owned(),
        ));
    }
    let json = connection.query_row(
        "SELECT value FROM job_metadata WHERE key = 'split_report_snapshot'",
        [],
        |row| row.get::<_, String>(0),
    )?;
    let expected_sha256 = connection
        .query_row(
            "SELECT value FROM job_metadata WHERE key = 'split_report_snapshot_sha256'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| JobError::Integrity("report snapshot has no integrity digest".to_owned()))?;
    let actual_sha256 = digest_hex(Sha256::digest(json.as_bytes()).as_slice());
    if actual_sha256 != expected_sha256 {
        return Err(JobError::Integrity(
            "report snapshot failed SHA-256 validation".to_owned(),
        ));
    }
    let expected_state_sha256 = connection
        .query_row(
            "SELECT value FROM job_metadata WHERE key = 'split_report_state_sha256'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| JobError::Integrity("report snapshot has no ledger digest".to_owned()))?;
    let evidence = report_ledger_evidence(&connection)?;
    if digest_report_ledger_evidence(&evidence)? != expected_state_sha256 {
        return Err(JobError::Integrity(
            "report snapshot disagrees with current durable job state".to_owned(),
        ));
    }
    let parts = read_published_parts(&connection)?;
    Ok(ValidatedReportSnapshot {
        json,
        parts,
        evidence,
    })
}

fn read_published_parts(connection: &Connection) -> Result<Vec<PublishedPartRecord>, JobError> {
    let mut statement = connection.prepare(
        "SELECT p.part_index, p.filename, p.byte_len, p.sha256, p.oversize, \
                p.sidecar_json, COUNT(i.item_key) \
         FROM parts p JOIN part_items i ON i.part_index = p.part_index \
         GROUP BY p.part_index, p.filename, p.byte_len, p.sha256, p.oversize, p.sidecar_json \
         ORDER BY p.part_index",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                PublishedPart {
                    index: row.get(0)?,
                    filename: row.get(1)?,
                    byte_len: row.get(2)?,
                    sha256: row.get(3)?,
                    oversize: row.get(4)?,
                },
                row.get::<_, String>(5)?,
                row.get::<_, u64>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(part, sidecar, item_count)| {
            let sidecar = serde_json::from_str(&sidecar)?;
            validate_sidecar(&part, &sidecar)?;
            Ok(PublishedPartRecord {
                part,
                sidecar,
                item_count,
            })
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct ReportLedgerEvidence {
    pub source: JobSourceIdentity,
    pub configuration: JobConfiguration,
    pub recovery_completion: Option<RecoveryCompletion>,
    pub summary: JobSummary,
    pub rejection_counts: CandidateRejectionCounts,
    pub worker_attempts: u32,
    pub worker_failures: u32,
    pub worker_retries_exhausted: bool,
    pub isolated_units: u64,
    pub interrupted: bool,
    pub direct_terminal_failure: Option<String>,
}

fn report_ledger_digest(connection: &Connection) -> Result<String, JobError> {
    digest_report_ledger_evidence(&report_ledger_evidence(connection)?)
}

fn report_ledger_evidence(connection: &Connection) -> Result<ReportLedgerEvidence, JobError> {
    let source = read_metadata_json(connection, "source_identity")?;
    let configuration = read_metadata_json(connection, "job_configuration")?;
    let recovery_completion = connection
        .query_row(
            "SELECT value FROM job_metadata WHERE key = 'recovery_completion'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| serde_json::from_str(&value))
        .transpose()?;
    let (committed, recovered, orphan, fragment, complete, partial, unsupported) = connection
        .query_row(
            "SELECT COUNT(*),\
                COALESCE(SUM(provenance = 'recovered'), 0),\
                COALESCE(SUM(provenance = 'orphan'), 0),\
                COALESCE(SUM(provenance = 'fragment'), 0),\
                COALESCE(SUM(completeness = 'complete'), 0),\
                COALESCE(SUM(completeness = 'partial'), 0),\
                COALESCE(SUM(status = 'unsupported'), 0)\
         FROM candidates WHERE status IN ('spooled', 'written', 'unsupported')",
            [],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, u64>(6)?,
                ))
            },
        )?;
    let (live_blob_count, live_blob_bytes) = connection.query_row(
        "SELECT COUNT(*), COALESCE(SUM(byte_len), 0) FROM blobs",
        [],
        |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
    )?;
    let final_blob_count = read_optional_metadata_u64(connection, "final_blob_count")?;
    let final_blob_bytes = read_optional_metadata_u64(connection, "final_blob_bytes")?;
    let (blob_count, blob_bytes) = match (final_blob_count, final_blob_bytes) {
        (Some(count), Some(bytes)) if live_blob_count == 0 && live_blob_bytes == 0 => {
            (count, bytes)
        }
        (None, None) => (live_blob_count, live_blob_bytes),
        _ => {
            return Err(JobError::Integrity(
                "final spool metrics disagree with retained work".to_owned(),
            ));
        }
    };
    let interrupted = read_strict_boolean_metadata(connection, "interrupted")?;
    let worker_retries_exhausted =
        read_strict_boolean_metadata(connection, "worker_retries_exhausted")?;
    let direct_terminal_failure = connection
        .query_row(
            "SELECT value FROM job_metadata WHERE key = 'direct_terminal_failure'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if direct_terminal_failure
        .as_deref()
        .is_some_and(|value| !matches!(value, "worker_crash" | "worker_stall" | "worker_protocol"))
    {
        return Err(JobError::Integrity(
            "direct terminal failure metadata is invalid".to_owned(),
        ));
    }
    let evidence = ReportLedgerEvidence {
        source,
        configuration,
        recovery_completion,
        summary: JobSummary {
            committed_candidates: committed,
            recovered_candidates: recovered,
            orphan_candidates: orphan,
            fragment_candidates: fragment,
            complete_candidates: complete,
            partial_candidates: partial,
            unsupported_candidates: unsupported,
            blob_count,
            blob_bytes,
        },
        rejection_counts: read_candidate_rejection_counts(connection)?,
        worker_attempts: read_optional_metadata_u32(connection, "worker_attempts")?,
        worker_failures: read_optional_metadata_u32(connection, "worker_failures")?,
        worker_retries_exhausted,
        isolated_units: connection.query_row("SELECT COUNT(*) FROM isolated_units", [], |row| {
            row.get::<_, u64>(0)
        })?,
        interrupted,
        direct_terminal_failure,
    };
    Ok(evidence)
}

fn digest_report_ledger_evidence(evidence: &ReportLedgerEvidence) -> Result<String, JobError> {
    Ok(digest_hex(
        Sha256::digest(serde_json::to_vec(evidence)?).as_slice(),
    ))
}

fn read_strict_boolean_metadata(
    connection: &Connection,
    key: &'static str,
) -> Result<bool, JobError> {
    let value = connection
        .query_row(
            "SELECT value FROM job_metadata WHERE key = ?1",
            [key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match value.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(JobError::Integrity(format!("invalid {key}"))),
    }
}

fn invalidate_report_snapshot(connection: &Connection) -> Result<(), JobError> {
    connection.execute(
        "DELETE FROM job_metadata WHERE key IN (\
            'split_report_snapshot',\
            'split_report_snapshot_sha256',\
            'split_report_state_sha256'\
         )",
        [],
    )?;
    Ok(())
}

impl CatalogSink for DurableCatalogSink {
    fn property_payload(&self, descriptor: PropertyDescriptor) -> PayloadRequest {
        match self.capture_mode {
            CatalogCaptureMode::Full => PayloadRequest::Full,
            CatalogCaptureMode::Bounded {
                property_prefix_bytes,
                ..
            } => PayloadRequest::Prefix(descriptor.data_size.min(property_prefix_bytes)),
        }
    }

    fn attachment_payload(
        &self,
        _message_id: u32,
        _index: u32,
        declared_size: Option<u64>,
    ) -> PayloadRequest {
        match self.capture_mode {
            CatalogCaptureMode::Full => PayloadRequest::Full,
            CatalogCaptureMode::Bounded {
                attachment_prefix_bytes,
                ..
            } => PayloadRequest::Prefix(
                declared_size
                    .unwrap_or_default()
                    .min(attachment_prefix_bytes),
            ),
        }
    }

    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
        self.handle_event(event).map_err(|error| error.to_string())
    }
}

impl DurableCatalogSink {
    pub fn record_replayed_candidate(
        &mut self,
        candidate: &ReplayCandidate,
        observed_id: u32,
    ) -> Result<(), JobError> {
        if self.active.is_some() || self.unit != candidate.unit {
            return Err(JobError::EventSequence(
                "replayed candidate is outside its durable recovery unit".to_owned(),
            ));
        }
        let embedded_path = candidate.metadata["embedded_path"]
            .as_array()
            .ok_or_else(|| {
                JobError::Integrity("replayed candidate embedded path is invalid".to_owned())
            })?
            .iter()
            .map(|value| {
                value.as_u64().ok_or_else(|| {
                    JobError::Integrity(
                        "replayed candidate embedded path element is invalid".to_owned(),
                    )
                })
            })
            .map(|value| {
                value.and_then(|value| {
                    u32::try_from(value).map_err(|_| {
                        JobError::Integrity(
                            "replayed candidate embedded path element is too large".to_owned(),
                        )
                    })
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.recent_candidates
            .insert(embedded_path, candidate.item_key.clone());
        self.replayed_source_ids.insert(observed_id, candidate.id);
        Ok(())
    }

    fn handle_event(&mut self, event: CatalogEvent<'_>) -> Result<(), JobError> {
        match event {
            CatalogEvent::UnitStart(unit) => {
                if self.active.is_some() || self.pending_named_property.is_some() {
                    return Err(JobError::EventSequence(
                        "recovery unit boundary occurred during a candidate".to_owned(),
                    ));
                }
                if self.unit.replace(unit).is_some() {
                    return Err(JobError::EventSequence(
                        "recovery units cannot be nested".to_owned(),
                    ));
                }
                self.recent_candidates.clear();
                self.replayed_source_ids.clear();
            }
            CatalogEvent::UnitEnd(unit) => {
                if self.active.is_some()
                    || self.pending_named_property.is_some()
                    || self.unit.take() != Some(unit)
                {
                    return Err(JobError::EventSequence(
                        "recovery unit ended out of sequence".to_owned(),
                    ));
                }
                self.replayed_source_ids.clear();
            }
            CatalogEvent::Folder {
                id,
                parent_id,
                name,
                container_class,
            } => {
                if self.active.is_some() || self.pending_named_property.is_some() {
                    return Err(JobError::EventSequence(
                        "folder event occurred during a message".to_owned(),
                    ));
                }
                let address = match self.unit {
                    Some(RecoveryUnit::Folder { address }) => Some(address),
                    _ => None,
                };
                let address_json = address
                    .map(|value| serde_json::to_string(&value))
                    .transpose()?;
                let folder_key = address_json
                    .clone()
                    .unwrap_or_else(|| format!("legacy:{id}"));
                self.connection.execute(
                    "INSERT OR REPLACE INTO folders(\
                        folder_key, source_id, parent_source_id, name, address_json, container_class\
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        folder_key,
                        i64::from(id),
                        parent_id.map(i64::from),
                        name,
                        address_json,
                        container_class
                    ],
                )?;
            }
            CatalogEvent::MessageStart {
                id,
                provenance,
                recovery_index,
                folder_id,
                parent_message_id,
                parent_attachment_index,
                embedded_path,
                associated,
                item_type,
                message_class,
                subject,
                sender_name,
                sender_email,
                submit_filetime,
                delivery_filetime,
                supported,
            } => self.start_candidate(CandidateStart {
                metadata: json!({
                    "folder_id": folder_id,
                    "parent_message_id": parent_message_id,
                    "parent_attachment_index": parent_attachment_index,
                    "embedded_path": embedded_path,
                    "associated": associated,
                    "item_type": item_type,
                    "message_class": message_class,
                    "subject": subject,
                    "sender_name": sender_name,
                    "sender_email": sender_email,
                    "submit_filetime": submit_filetime,
                    "delivery_filetime": delivery_filetime,
                    "supported": supported,
                }),
                id,
                provenance,
                recovery_index,
                parent_message_id,
                parent_attachment_index,
                embedded_path,
                supported,
            })?,
            CatalogEvent::Recipient {
                message_id,
                index,
                recipient_type,
                display_name,
                email_address,
                address_type,
            } => {
                require_message(self.active.as_ref(), message_id)?;
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.recipients.contains(&index))
                {
                    return Err(JobError::EventSequence(
                        "duplicate recipient index".to_owned(),
                    ));
                }
                self.record_event(
                    "recipient",
                    json!({
                        "index": index,
                        "recipient_type": recipient_type,
                        "display_name": display_name,
                        "email_address": email_address,
                        "address_type": address_type,
                    }),
                    None,
                )?;
                self.active
                    .as_mut()
                    .ok_or_else(|| {
                        JobError::EventSequence("recipient occurred without a message".to_owned())
                    })?
                    .recipients
                    .insert(index);
            }
            CatalogEvent::AttachmentStart {
                message_id,
                index,
                attachment_type,
                data_size,
                filename,
            } => {
                require_message(self.active.as_ref(), message_id)?;
                if self.attachment.is_some() {
                    return Err(JobError::EventSequence(
                        "attachment started before previous attachment ended".to_owned(),
                    ));
                }
                self.record_event(
                    "attachment",
                    json!({
                        "index": index,
                        "attachment_type": attachment_type,
                        "data_size": data_size,
                        "filename": filename,
                    }),
                    None,
                )?;
                self.attachment = Some(ActiveAttachment {
                    message_id,
                    index,
                    attachment_type,
                    expected: data_size,
                    blob: None,
                });
            }
            CatalogEvent::AttachmentData {
                message_id,
                index,
                bytes,
            } => {
                require_message(self.active.as_ref(), message_id)?;
                let needs_blob = self
                    .attachment
                    .as_ref()
                    .is_some_and(|attachment| attachment.blob.is_none());
                let new_blob = needs_blob.then(|| self.new_blob_writer()).transpose()?;
                let attachment = self.attachment.as_mut().ok_or_else(|| {
                    JobError::EventSequence("attachment data without metadata".to_owned())
                })?;
                if attachment.message_id != message_id || attachment.index != index {
                    return Err(JobError::EventSequence(
                        "attachment data does not match active attachment".to_owned(),
                    ));
                }
                if let Some(blob) = new_blob {
                    attachment.blob = Some(blob);
                }
                attachment
                    .blob
                    .as_mut()
                    .ok_or_else(|| JobError::EventSequence("attachment blob missing".to_owned()))?
                    .write(
                        &mut self.payload_pack,
                        &self.payload_pack_path,
                        &self.payload_pack_position,
                        &self.payload_pack_bytes_written,
                        &self.payload_pack_peak_bytes,
                        bytes,
                    )?;
            }
            CatalogEvent::AttachmentEnd { message_id, index } => {
                require_message(self.active.as_ref(), message_id)?;
                let attachment = self.attachment.as_ref().ok_or_else(|| {
                    JobError::EventSequence("attachment end without metadata".to_owned())
                })?;
                if attachment.message_id != message_id || attachment.index != index {
                    return Err(JobError::EventSequence(
                        "attachment end does not match active attachment".to_owned(),
                    ));
                }
                self.finish_attachment(true)?;
            }
            CatalogEvent::AttachmentAbort { message_id, index } => {
                require_message(self.active.as_ref(), message_id)?;
                let attachment = self.attachment.as_ref().ok_or_else(|| {
                    JobError::EventSequence("attachment abort without metadata".to_owned())
                })?;
                if attachment.message_id != message_id || attachment.index != index {
                    return Err(JobError::EventSequence(
                        "attachment abort does not match active attachment".to_owned(),
                    ));
                }
                self.finish_attachment(false)?;
            }
            CatalogEvent::PropertyStart(descriptor) => {
                if self.property.is_some() {
                    return Err(JobError::EventSequence(
                        "property started before previous property ended".to_owned(),
                    ));
                }
                validate_property_owner(
                    self.active.as_ref(),
                    self.attachment.as_ref(),
                    descriptor.owner,
                )?;
                let named_property = match self.pending_named_property.take() {
                    Some((pending, identity)) if pending == descriptor => Some(identity),
                    Some(_) => {
                        return Err(JobError::EventSequence(
                            "named property identity does not match property start".to_owned(),
                        ));
                    }
                    None => None,
                };
                self.property = Some(ActiveProperty {
                    descriptor,
                    named_property,
                    blob: self.new_blob_writer()?,
                    record: !matches!(descriptor.owner, PropertyOwner::Folder(_)),
                });
            }
            CatalogEvent::NamedProperty {
                descriptor,
                identity,
            } => {
                if self.property.is_some() || self.pending_named_property.is_some() {
                    return Err(JobError::EventSequence(
                        "named property identity occurred out of sequence".to_owned(),
                    ));
                }
                validate_property_owner(
                    self.active.as_ref(),
                    self.attachment.as_ref(),
                    descriptor.owner,
                )?;
                self.pending_named_property = Some((descriptor, identity));
            }
            CatalogEvent::PropertyData { descriptor, bytes } => {
                let property = self.property.as_mut().ok_or_else(|| {
                    JobError::EventSequence("property data without start".to_owned())
                })?;
                if property.descriptor != descriptor {
                    return Err(JobError::EventSequence(
                        "property data does not match active property".to_owned(),
                    ));
                }
                property.blob.write(
                    &mut self.payload_pack,
                    &self.payload_pack_path,
                    &self.payload_pack_position,
                    &self.payload_pack_bytes_written,
                    &self.payload_pack_peak_bytes,
                    bytes,
                )?;
            }
            CatalogEvent::PropertyEnd(descriptor) => self.finish_property(descriptor)?,
            CatalogEvent::PropertyAbort { descriptor, reason } => {
                let property = self.property.take().ok_or_else(|| {
                    JobError::EventSequence("property abort without start".to_owned())
                })?;
                if property.descriptor != descriptor {
                    return Err(JobError::EventSequence(
                        "property abort does not match active property".to_owned(),
                    ));
                }
                self.truncate_payload_pack(property.blob.start_offset)?;
                if property.record {
                    self.record_event(
                        "property_incomplete",
                        json!({
                            "property": property_json(
                                property.descriptor,
                                property.named_property.as_ref(),
                            ),
                            "reason": reason,
                        }),
                        None,
                    )?;
                }
            }
            CatalogEvent::MessageEnd { id, complete } => {
                require_message(self.active.as_ref(), id)?;
                if self.pending_named_property.is_some() {
                    return Err(JobError::EventSequence(
                        "message ended after an unmatched named property identity".to_owned(),
                    ));
                }
                if complete && self.property.is_some() {
                    return Err(JobError::EventSequence(
                        "message ended during a property".to_owned(),
                    ));
                }
                if complete && self.attachment.is_some() {
                    return Err(JobError::EventSequence(
                        "message ended before attachment end".to_owned(),
                    ));
                }
                if !complete {
                    if let Some(property) = self.property.take() {
                        self.truncate_payload_pack(property.blob.start_offset)?;
                        self.record_event(
                            "property_incomplete",
                            json!({
                                "property": property_json(
                                    property.descriptor,
                                    property.named_property.as_ref(),
                                ),
                                "reason": "message ended before property completion",
                            }),
                            None,
                        )?;
                    }
                }
                if !complete {
                    self.finish_attachment(false)?;
                }
                if self.active.as_ref().is_some_and(|active| !active.supported) {
                    self.record_event(
                        "output_unrepresentable",
                        serde_json::to_value(CandidateRejectionMetadata {
                            schema_version: 1,
                            category: CandidateRejectionCategory::SourceItemUnsupported,
                        })?,
                        None,
                    )?;
                }
                let active = self.active.take().ok_or_else(|| {
                    JobError::EventSequence("message ended without start".to_owned())
                })?;
                self.connection.execute(
                    "UPDATE candidates SET completeness = ?1, status = ?2 WHERE item_key = ?3",
                    params![
                        if complete { "complete" } else { "partial" },
                        if active.supported {
                            "spooled"
                        } else {
                            "unsupported"
                        },
                        active.key
                    ],
                )?;
                if let Err(error) = self.finish_candidate() {
                    let _ = self.connection.execute_batch("ROLLBACK");
                    let _ = self.truncate_payload_pack(self.batch_pack_start.get());
                    self.batch_open.set(false);
                    self.batch_candidates.set(0);
                    return Err(error);
                }
                self.recent_candidates
                    .insert(active.embedded_path, active.key.clone());
            }
            CatalogEvent::DeferredPropertyData { .. }
            | CatalogEvent::DeferredAttachmentData { .. }
            | CatalogEvent::TopLevelMetadataEnd
            | CatalogEvent::TopLevelPayloadEnd => {
                return Err(JobError::EventSequence(
                    "deferred direct payload event reached the durable metadata sink".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

impl Drop for DurableCatalogSink {
    fn drop(&mut self) {
        self.rollback();
    }
}

impl BlobWriter {
    fn new(start_offset: u64) -> Self {
        Self {
            start_offset,
            hasher: Sha256::new(),
            bytes: 0,
            inline: None,
        }
    }

    fn new_inline() -> Self {
        Self {
            start_offset: 0,
            hasher: Sha256::new(),
            bytes: 0,
            inline: Some(Vec::new()),
        }
    }

    fn write(
        &mut self,
        pack: &mut File,
        path: &Path,
        pack_position: &Cell<u64>,
        pack_bytes_written: &Cell<u64>,
        pack_peak_bytes: &Cell<u64>,
        bytes: &[u8],
    ) -> Result<(), JobError> {
        if let Some(inline) = self.inline.as_mut() {
            inline.extend_from_slice(bytes);
            self.hasher.update(bytes);
            self.bytes = self
                .bytes
                .checked_add(
                    u64::try_from(bytes.len())
                        .map_err(|_| JobError::Integrity("inline length overflow".to_owned()))?,
                )
                .ok_or_else(|| JobError::Integrity("inline length overflow".to_owned()))?;
            Ok(())
        } else {
            let before = self.bytes;
            let result = write_hashed(pack, &mut self.hasher, &mut self.bytes, bytes);
            let written = self.bytes.saturating_sub(before);
            let position = pack_position
                .get()
                .checked_add(written)
                .ok_or_else(|| JobError::Integrity("payload pack offset overflow".to_owned()))?;
            pack_position.set(position);
            pack_bytes_written.set(pack_bytes_written.get().saturating_add(written));
            pack_peak_bytes.set(pack_peak_bytes.get().max(position));
            result.map_err(|source| io_error(path, source))
        }
    }
}

fn verify_blob_bytes(expected_hash: &str, expected_len: u64, data: &[u8]) -> Result<(), JobError> {
    if u64::try_from(data.len()).ok() != Some(expected_len)
        || digest_hex(Sha256::digest(data).as_slice()) != expected_hash
    {
        return Err(JobError::Integrity(format!(
            "inline blob {expected_hash} failed SHA-256 validation"
        )));
    }
    Ok(())
}

fn write_hashed<W: std::io::Write>(
    writer: &mut W,
    hasher: &mut Sha256,
    total: &mut u64,
    mut bytes: &[u8],
) -> std::io::Result<()> {
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => return Err(std::io::Error::from(ErrorKind::WriteZero)),
            Ok(written) => {
                hasher.update(&bytes[..written]);
                *total = total
                    .checked_add(u64::try_from(written).map_err(std::io::Error::other)?)
                    .ok_or_else(|| std::io::Error::other("blob length overflow"))?;
                bytes = &bytes[written..];
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

struct BlobRef {
    sha256: String,
    bytes: u64,
}

fn read_schema_version(connection: &Connection) -> Result<i64, JobError> {
    connection
        .query_row(
            "SELECT value FROM job_metadata WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )?
        .parse::<i64>()
        .map_err(|_| JobError::Integrity("invalid schema version".to_owned()))
}

fn read_metadata_json<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    key: &'static str,
) -> Result<T, JobError> {
    let value = connection
        .query_row(
            "SELECT value FROM job_metadata WHERE key = ?1",
            [key],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                JobError::ResumeMismatch("required immutable metadata")
            }
            other => other.into(),
        })?;
    Ok(serde_json::from_str(&value)?)
}

fn read_candidate_rejection_counts(
    connection: &Connection,
) -> Result<CandidateRejectionCounts, JobError> {
    let mut statement = connection.prepare(
        "SELECT candidates.item_key, candidates.status, candidate_events.metadata_json \
         FROM candidates \
         LEFT JOIN candidate_events \
           ON candidate_events.item_key = candidates.item_key \
          AND candidate_events.kind = 'output_unrepresentable' \
         WHERE candidates.status = 'unsupported' \
            OR candidate_events.item_key IS NOT NULL \
         ORDER BY candidates.item_key, candidate_events.sequence",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut counts = BTreeMap::new();
    let mut seen = HashSet::new();
    for row in rows {
        let (item_key, status, metadata_json) = row?;
        if !seen.insert(item_key.clone()) {
            return Err(JobError::Integrity(format!(
                "unsupported candidate {item_key} has duplicate rejection events"
            )));
        }
        if status != "unsupported" {
            return Err(JobError::Integrity(format!(
                "candidate {item_key} has a rejection event but status {status:?}"
            )));
        }
        let metadata_json = metadata_json.ok_or_else(|| {
            JobError::Integrity(format!(
                "unsupported candidate {item_key} has no rejection event"
            ))
        })?;
        let metadata: CandidateRejectionMetadata =
            serde_json::from_str(&metadata_json).map_err(|error| {
                JobError::Integrity(format!(
                    "unsupported candidate {item_key} has invalid rejection metadata: {error}"
                ))
            })?;
        if metadata.schema_version != 1 {
            return Err(JobError::Integrity(format!(
                "unsupported candidate {item_key} has unsupported rejection schema {}",
                metadata.schema_version
            )));
        }
        let count = counts.entry(metadata.category).or_insert(0_u64);
        *count = count.saturating_add(1);
    }
    Ok(counts)
}

fn validate_resume_metadata(
    connection: &Connection,
    source: &JobSourceIdentity,
    configuration: &JobConfiguration,
) -> Result<(), JobError> {
    let stored_source: JobSourceIdentity = read_metadata_json(connection, "source_identity")?;
    if &stored_source != source {
        return Err(JobError::ResumeMismatch("source identity or SHA-256"));
    }
    let stored_configuration: JobConfiguration =
        read_metadata_json(connection, "job_configuration")?;
    if stored_configuration.tool_compatibility_major != configuration.tool_compatibility_major {
        return Err(JobError::ResumeMismatch("tool compatibility major version"));
    }
    if stored_configuration.split_schema_version != configuration.split_schema_version {
        return Err(JobError::ResumeMismatch("split report schema version"));
    }
    if stored_configuration.execution_mode != configuration.execution_mode {
        return Err(JobError::ResumeMismatch("execution mode"));
    }
    if stored_configuration.recovery_mode != configuration.recovery_mode {
        return Err(JobError::ResumeMismatch("recovery mode"));
    }
    if stored_configuration.maximum_pst_bytes != configuration.maximum_pst_bytes
        || stored_configuration.part_size_policy != configuration.part_size_policy
    {
        return Err(JobError::ResumeMismatch("part-size policy"));
    }
    if stored_configuration.writer_format != configuration.writer_format {
        return Err(JobError::ResumeMismatch("writer format"));
    }
    Ok(())
}

fn allocated_tree_bytes(path: &Path, held_root: bool) -> Result<u64, JobError> {
    let metadata = if held_root {
        path.metadata()
    } else {
        path.symlink_metadata()
    }
    .map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink()
        || (!metadata.is_dir() && !metadata.is_file())
        || metadata.uid() != rustix::process::getuid().as_raw()
    {
        return Err(JobError::UnsafePath(path.to_path_buf()));
    }
    let mut bytes = metadata.blocks().saturating_mul(512);
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|source| io_error(path, source))? {
            let entry = entry.map_err(|source| io_error(path, source))?;
            bytes = bytes.saturating_add(allocated_tree_bytes(&entry.path(), false)?);
        }
    }
    Ok(bytes)
}

fn allocated_private_bytes(private_root: &Path) -> Result<u64, JobError> {
    let mut bytes = allocated_node_bytes(private_root, true)?;
    for entry in fs::read_dir(private_root).map_err(|source| io_error(private_root, source))? {
        let entry = entry.map_err(|source| io_error(private_root, source))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| JobError::UnsafePath(entry.path()))?;
        if !matches!(
            name.as_str(),
            "job.sqlite3"
                | "job.sqlite3-wal"
                | "job.sqlite3-shm"
                | "spool"
                | "partial"
                | "manifests"
        ) {
            return Err(JobError::UnsafePath(entry.path()));
        }
        bytes = bytes.saturating_add(if name == "partial" {
            // Validator-failure scratch is retained as evidence, not trusted output.
            let metadata = entry
                .path()
                .symlink_metadata()
                .map_err(|source| io_error(&entry.path(), source))?;
            if !metadata.is_dir() {
                return Err(JobError::UnsafePath(entry.path()));
            }
            allocated_node_bytes(&entry.path(), false)?
        } else {
            allocated_tree_bytes(&entry.path(), false)?
        });
    }
    Ok(bytes)
}

fn allocated_tracked_part_bytes(connection: &Connection, parts: &Path) -> Result<u64, JobError> {
    let mut bytes = allocated_node_bytes(parts, true)?;
    let mut statement = connection.prepare("SELECT filename FROM parts ORDER BY part_index")?;
    let filenames = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for filename in filenames {
        bytes = bytes.saturating_add(allocated_node_bytes(&parts.join(filename), false)?);
    }
    Ok(bytes)
}

fn path_exists(path: &Path) -> Result<bool, JobError> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error(path, source)),
    }
}

fn allocated_node_bytes(path: &Path, held_root: bool) -> Result<u64, JobError> {
    let metadata = if held_root {
        path.metadata()
    } else {
        path.symlink_metadata()
    }
    .map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink()
        || (!metadata.is_dir() && !metadata.is_file())
        || metadata.uid() != rustix::process::getuid().as_raw()
    {
        return Err(JobError::UnsafePath(path.to_path_buf()));
    }
    Ok(metadata.blocks().saturating_mul(512))
}

fn validate_private_root_entries(private_root: &Path) -> Result<(), JobError> {
    for entry in fs::read_dir(private_root).map_err(|source| io_error(private_root, source))? {
        let entry = entry.map_err(|source| io_error(private_root, source))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| JobError::UnsafePath(entry.path()))?;
        if !matches!(
            name.as_str(),
            "job.sqlite3"
                | "job.sqlite3-wal"
                | "job.sqlite3-shm"
                | "spool"
                | "partial"
                | "manifests"
        ) {
            return Err(JobError::UnsafePath(entry.path()));
        }
    }
    Ok(())
}

fn read_optional_metadata_u32(connection: &Connection, key: &'static str) -> Result<u32, JobError> {
    let value = connection
        .query_row(
            "SELECT value FROM job_metadata WHERE key = ?1",
            [key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    value.map_or(Ok(0), |value| {
        value
            .parse::<u32>()
            .map_err(|_| JobError::Integrity(format!("invalid {key}")))
    })
}

fn read_optional_metadata_u64(
    connection: &Connection,
    key: &'static str,
) -> Result<Option<u64>, JobError> {
    let value = connection
        .query_row(
            "SELECT value FROM job_metadata WHERE key = ?1",
            [key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    value
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| JobError::Integrity(format!("invalid {key}")))
        })
        .transpose()
}

fn configure(connection: &Connection) -> Result<(), JobError> {
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;\
         PRAGMA synchronous = FULL;\
         PRAGMA foreign_keys = ON;\
         PRAGMA trusted_schema = OFF;\
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(())
}

fn configure_capture_mode(
    connection: &Connection,
    capture_mode: CatalogCaptureMode,
) -> Result<(), JobError> {
    if matches!(capture_mode, CatalogCaptureMode::Bounded { .. }) {
        connection.execute_batch(&format!(
            "PRAGMA cache_size = -{DIRECT_SQLITE_CACHE_KIB}; PRAGMA cache_spill = ON;"
        ))?;
    }
    Ok(())
}

fn run_sql_interruptible<T>(
    connection: &mut Connection,
    interrupted: Option<&AtomicBool>,
    operation: impl FnOnce(&mut Connection) -> Result<T, JobError>,
) -> Result<T, JobError> {
    let Some(interrupted) = interrupted else {
        return operation(connection);
    };
    if interrupted.load(Ordering::Relaxed) {
        return Err(JobError::Interrupted);
    }
    let handle = connection.get_interrupt_handle();
    let finished = AtomicBool::new(false);
    let result = thread::scope(|scope| {
        scope.spawn(|| {
            while !finished.load(Ordering::Relaxed) {
                if interrupted.load(Ordering::Relaxed) {
                    handle.interrupt();
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        let result = operation(connection);
        finished.store(true, Ordering::Relaxed);
        result
    });
    if interrupted.load(Ordering::Relaxed) {
        return Err(JobError::Interrupted);
    }
    match result {
        Err(JobError::Sql(error))
            if error.sqlite_error_code() == Some(ErrorCode::OperationInterrupted) =>
        {
            Err(JobError::Interrupted)
        }
        other => other,
    }
}

fn ensure_inline_blob_table(connection: &Connection) -> Result<(), JobError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS inline_blobs(\
            sha256 TEXT PRIMARY KEY REFERENCES blobs(sha256) ON DELETE CASCADE,\
            data BLOB NOT NULL\
         ) STRICT;\
         CREATE INDEX IF NOT EXISTS candidate_events_blob_sha256 \
         ON candidate_events(blob_sha256) WHERE blob_sha256 IS NOT NULL;\
         CREATE INDEX IF NOT EXISTS candidates_occurrence \
         ON candidates(provenance, source_node_id, recovery_index);\
         CREATE INDEX IF NOT EXISTS candidates_parent_item_key \
         ON candidates(parent_item_key) WHERE parent_item_key IS NOT NULL;",
    )?;
    Ok(())
}

fn ensure_pack_offset_column(connection: &Connection) -> Result<(), JobError> {
    let mut statement = connection.prepare("PRAGMA table_info(blobs)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|column| column == "pack_offset") {
        connection.execute_batch(
            "ALTER TABLE blobs ADD COLUMN pack_offset INTEGER CHECK(pack_offset >= 0);",
        )?;
    }
    Ok(())
}

fn reconcile_payload_pack(
    connection: &Connection,
    pack: &mut File,
    path: &Path,
) -> Result<(), JobError> {
    let overlapping = connection.query_row(
        "SELECT EXISTS(\
             SELECT 1 FROM (\
                 SELECT pack_offset,\
                        LAG(pack_offset + byte_len) OVER (\
                            ORDER BY pack_offset, sha256\
                        ) AS previous_end \
                 FROM blobs WHERE pack_offset IS NOT NULL AND byte_len > 0\
             ) WHERE pack_offset < previous_end\
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if overlapping {
        return Err(JobError::Integrity(
            "payload pack ledger contains overlapping ranges".to_owned(),
        ));
    }
    let committed_end = connection.query_row(
        "SELECT COALESCE(MAX(pack_offset + byte_len), 0) FROM blobs \
         WHERE pack_offset IS NOT NULL",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    let actual = pack
        .metadata()
        .map_err(|source| io_error(path, source))?
        .len();
    if actual < committed_end {
        return Err(JobError::Integrity(
            "payload pack is shorter than its committed ledger ranges".to_owned(),
        ));
    }
    if actual > committed_end {
        pack.set_len(committed_end)
            .map_err(|source| io_error(path, source))?;
        pack.sync_all().map_err(|source| io_error(path, source))?;
    }
    pack.seek(std::io::SeekFrom::End(0))
        .map_err(|source| io_error(path, source))?;
    Ok(())
}

fn create_schema(connection: &Connection) -> Result<(), JobError> {
    connection.execute_batch(
        "CREATE TABLE job_metadata(\
            key TEXT PRIMARY KEY, value TEXT NOT NULL\
         ) STRICT;\
         CREATE TABLE folders(\
            folder_key TEXT PRIMARY KEY,\
            source_id INTEGER NOT NULL,\
            parent_source_id INTEGER,\
            name TEXT,\
            address_json TEXT,\
            container_class TEXT\
         ) STRICT;\
         CREATE TABLE candidates(\
            item_key TEXT PRIMARY KEY,\
            provenance TEXT NOT NULL CHECK(provenance IN ('normal','recovered','orphan','fragment')),\
            source_node_id INTEGER,\
            recovery_index INTEGER,\
            occurrence INTEGER NOT NULL CHECK(occurrence >= 0),\
            completeness TEXT NOT NULL CHECK(completeness IN ('complete','partial','damaged')),\
            status TEXT NOT NULL CHECK(status IN ('pending','spooled','written','unsupported','failed')),\
            metadata_json TEXT NOT NULL,\
            recovery_unit_json TEXT,\
            parent_item_key TEXT REFERENCES candidates(item_key),\
            parent_attachment_index INTEGER CHECK(parent_attachment_index >= 0),\
            embedded_path_json TEXT NOT NULL DEFAULT '[]',\
            CHECK((parent_item_key IS NULL) = (parent_attachment_index IS NULL))\
         ) STRICT;\
         CREATE TABLE blobs(\
            sha256 TEXT PRIMARY KEY CHECK(\
                length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'\
            ),\
            byte_len INTEGER NOT NULL CHECK(byte_len >= 0),\
            pack_offset INTEGER CHECK(pack_offset >= 0)\
         ) STRICT;\
         CREATE TABLE inline_blobs(\
            sha256 TEXT PRIMARY KEY REFERENCES blobs(sha256) ON DELETE CASCADE,\
            data BLOB NOT NULL\
         ) STRICT;\
         CREATE TABLE candidate_events(\
            item_key TEXT NOT NULL REFERENCES candidates(item_key) ON DELETE CASCADE,\
            sequence INTEGER NOT NULL CHECK(sequence > 0),\
            kind TEXT NOT NULL,\
            metadata_json TEXT NOT NULL,\
            blob_sha256 TEXT REFERENCES blobs(sha256),\
            byte_len INTEGER CHECK(byte_len >= 0),\
            CHECK((blob_sha256 IS NULL) = (byte_len IS NULL)),\
            PRIMARY KEY(item_key, sequence)\
         ) STRICT;\
         CREATE INDEX candidate_events_blob_sha256 \
         ON candidate_events(blob_sha256) WHERE blob_sha256 IS NOT NULL;\
         CREATE INDEX candidates_occurrence \
         ON candidates(provenance, source_node_id, recovery_index);\
         CREATE INDEX candidates_parent_item_key \
         ON candidates(parent_item_key) WHERE parent_item_key IS NOT NULL;\
         CREATE TABLE worker_events(\
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,\
            kind TEXT NOT NULL CHECK(kind IN ('started','failure')),\
            attempt INTEGER NOT NULL CHECK(attempt > 0),\
            category TEXT NOT NULL\
         ) STRICT;\
         CREATE TABLE isolated_units(\
            unit_json TEXT PRIMARY KEY,\
            failures INTEGER NOT NULL CHECK(failures > 0)\
         ) STRICT;\
         CREATE TABLE parts(\
            part_index INTEGER PRIMARY KEY CHECK(part_index > 0),\
            filename TEXT NOT NULL UNIQUE,\
            byte_len INTEGER NOT NULL CHECK(byte_len > 0),\
            sha256 TEXT CHECK(sha256 IS NULL OR (\
                length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'\
            )),\
            oversize INTEGER NOT NULL CHECK(oversize IN (0, 1)),\
            sidecar_json TEXT NOT NULL\
         ) STRICT;\
         CREATE TABLE part_items(\
            part_index INTEGER NOT NULL REFERENCES parts(part_index),\
            item_key TEXT NOT NULL UNIQUE REFERENCES candidates(item_key),\
            PRIMARY KEY(part_index, item_key)\
         ) STRICT;\
         CREATE TABLE publication_intents(\
            part_index INTEGER PRIMARY KEY CHECK(part_index > 0),\
            part_json TEXT NOT NULL,\
            sidecar_json TEXT NOT NULL,\
            item_keys_json TEXT NOT NULL\
         ) STRICT;",
    )?;
    Ok(())
}

fn property_json(
    descriptor: PropertyDescriptor,
    named_property: Option<&NamedPropertyIdentity>,
) -> serde_json::Value {
    let (owner, owner_id, index) = match descriptor.owner {
        PropertyOwner::Folder(id) => ("folder", id, None),
        PropertyOwner::Message(id) => ("message", id, None),
        PropertyOwner::Recipient { message_id, index } => ("recipient", message_id, Some(index)),
        PropertyOwner::Attachment { message_id, index } => ("attachment", message_id, Some(index)),
    };
    json!({
        "owner": owner,
        "owner_id": owner_id,
        "owner_index": index,
        "record_set_index": descriptor.record_set_index,
        "entry_index": descriptor.entry_index,
        "entry_type": descriptor.entry_type,
        "value_type": descriptor.value_type,
        "data_size": descriptor.data_size,
        "named_property": named_property,
    })
}

fn parse_provenance(value: &str) -> Result<CatalogProvenance, JobError> {
    match value {
        "normal" => Ok(CatalogProvenance::Normal),
        "recovered" => Ok(CatalogProvenance::Recovered),
        "orphan" => Ok(CatalogProvenance::Orphan),
        "fragment" => Ok(CatalogProvenance::Fragment),
        other => Err(JobError::Integrity(format!(
            "invalid candidate provenance {other:?}"
        ))),
    }
}

fn checked_u32(value: i64, name: &str) -> Result<u32, JobError> {
    u32::try_from(value).map_err(|_| JobError::Integrity(format!("invalid {name}")))
}

fn checked_u64(value: i64, name: &str) -> Result<u64, JobError> {
    u64::try_from(value).map_err(|_| JobError::Integrity(format!("invalid {name}")))
}

fn commit_published_part_transaction(
    connection: &mut Connection,
    part: &PublishedPart,
    sidecar: &PartSidecar,
    item_keys: &[String],
    clear_intent: bool,
) -> Result<(), JobError> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO parts(part_index, filename, byte_len, sha256, oversize, sidecar_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            part.index,
            part.filename,
            part.byte_len,
            part.sha256,
            part.oversize,
            serde_json::to_string(sidecar)?
        ],
    )?;
    for item_key in item_keys {
        let changed = transaction.execute(
            "UPDATE candidates SET status = 'written' \
             WHERE item_key = ?1 AND status = 'spooled'",
            [item_key],
        )?;
        if changed != 1 {
            return Err(JobError::Integrity(format!(
                "candidate {item_key} is not available for part assignment"
            )));
        }
        transaction.execute(
            "INSERT INTO part_items(part_index, item_key) VALUES (?1, ?2)",
            params![part.index, item_key],
        )?;
    }
    if clear_intent {
        let changed = transaction.execute(
            "DELETE FROM publication_intents WHERE part_index = ?1",
            [part.index],
        )?;
        if changed != 1 {
            return Err(JobError::Integrity(format!(
                "publication intent for part {} is absent",
                part.index
            )));
        }
    }
    transaction.commit()?;
    Ok(())
}

fn reconcile_publications(
    connection: &mut Connection,
    partial_directory: &File,
    manifests_directory: &File,
    partial: &Path,
    parts: &Path,
    manifests: &Path,
    interrupted: Option<&AtomicBool>,
) -> Result<(), JobError> {
    let intents = {
        let mut statement = connection.prepare(
            "SELECT part_json, sidecar_json, item_keys_json \
             FROM publication_intents ORDER BY part_index",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (part_json, sidecar_json, item_keys_json) in intents {
        let part: PublishedPart = serde_json::from_str(&part_json)?;
        let sidecar: PartSidecar = serde_json::from_str(&sidecar_json)?;
        let item_keys: Vec<String> = serde_json::from_str(&item_keys_json)?;
        validate_part_record(&part)?;
        validate_sidecar(&part, &sidecar)?;
        if item_keys.is_empty() {
            return Err(JobError::Integrity(format!(
                "publication intent for part {} has no candidates",
                part.index
            )));
        }
        let final_path = parts.join(&part.filename);
        let sidecar_filename = part.filename.trim_end_matches(".pst").to_owned() + ".json";
        let sidecar_path = manifests.join(&sidecar_filename);
        let staged_sidecar_filename = sidecar_filename.clone() + ".partial";
        let staged_sidecar_path = partial.join(&staged_sidecar_filename);
        let final_exists = final_path
            .try_exists()
            .map_err(|source| io_error(&final_path, source))?;
        let sidecar_exists = sidecar_path
            .try_exists()
            .map_err(|source| io_error(&sidecar_path, source))?;
        if !final_exists {
            if sidecar_exists {
                return Err(JobError::Integrity(format!(
                    "part {} has a sidecar without its PST",
                    part.index
                )));
            }
            connection.execute(
                "DELETE FROM publication_intents WHERE part_index = ?1",
                [part.index],
            )?;
            continue;
        }
        verify_part_artifact(&final_path, &part, &sidecar, interrupted)?;
        if sidecar_exists {
            verify_sidecar_artifact(&sidecar_path, &sidecar)?;
        } else {
            if staged_sidecar_path
                .try_exists()
                .map_err(|source| io_error(&staged_sidecar_path, source))?
            {
                verify_sidecar_artifact(&staged_sidecar_path, &sidecar)?;
            } else {
                write_sidecar_partial(&staged_sidecar_path, &sidecar)?;
            }
            rename_noclobber(
                partial_directory,
                Path::new(&staged_sidecar_filename),
                manifests_directory,
                Path::new(&sidecar_filename),
                &sidecar_path,
            )?;
            sync_file(manifests_directory, manifests)?;
            verify_sidecar_artifact(&sidecar_path, &sidecar)?;
        }
        commit_published_part_transaction(connection, &part, &sidecar, &item_keys, true)?;
    }
    Ok(())
}

fn validate_part_record(part: &PublishedPart) -> Result<(), JobError> {
    if part.index == 0
        || part.byte_len == 0
        || part.filename.is_empty()
        || Path::new(&part.filename)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(part.filename.as_str())
        || !part.filename.ends_with(".pst")
        || part.sha256.as_ref().is_some_and(|sha256| {
            sha256.len() != 64
                || !sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    {
        return Err(JobError::Integrity(
            "invalid published part accounting record".to_owned(),
        ));
    }
    Ok(())
}

fn validate_sidecar(part: &PublishedPart, sidecar: &PartSidecar) -> Result<(), JobError> {
    let identity_valid = match sidecar.schema_version.as_str() {
        "1.1.0" => sidecar.published_device.is_none() && sidecar.published_inode.is_none(),
        "1.2.0" => sidecar.published_device.is_some() && sidecar.published_inode.is_some(),
        _ => false,
    };
    if !identity_valid
        || sidecar.producer_version.is_empty()
        || sidecar.index != part.index
        || sidecar.filename != part.filename
        || sidecar.byte_len != part.byte_len
        || sidecar.sha256 != part.sha256
        || sidecar.oversize != part.oversize
        || sidecar.store_record_key.len() != 32
        || !sidecar
            .store_record_key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || sidecar.folder_count == 0
        || sidecar.message_count == 0
    {
        return Err(JobError::Integrity(
            "part sidecar disagrees with the publication record".to_owned(),
        ));
    }
    Ok(())
}

fn write_sidecar_partial(path: &Path, sidecar: &PartSidecar) -> Result<(), JobError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    serde_json::to_writer_pretty(&mut file, sidecar)?;
    file.write_all(b"\n")
        .map_err(|source| io_error(path, source))?;
    file.sync_all().map_err(|source| io_error(path, source))?;
    verify_sidecar_artifact(path, sidecar)
}

fn verify_sidecar_artifact(path: &Path, expected: &PartSidecar) -> Result<(), JobError> {
    let actual = read_sidecar_artifact(path)?;
    if &actual != expected {
        return Err(JobError::Integrity(
            "published part sidecar content mismatch".to_owned(),
        ));
    }
    Ok(())
}

fn read_sidecar_artifact(path: &Path) -> Result<PartSidecar, JobError> {
    let metadata = path
        .symlink_metadata()
        .map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink()
        || !private_state_attributes_valid(
            metadata.is_file(),
            metadata.uid(),
            metadata.mode(),
            Some(metadata.nlink()),
        )
    {
        return Err(JobError::Integrity(
            "published part sidecar has unsafe attributes".to_owned(),
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    let opened = file.metadata().map_err(|source| io_error(path, source))?;
    if metadata.dev() != opened.dev()
        || metadata.ino() != opened.ino()
        || !private_state_attributes_valid(
            opened.is_file(),
            opened.uid(),
            opened.mode(),
            Some(opened.nlink()),
        )
    {
        return Err(JobError::Integrity(
            "published part sidecar changed while opening".to_owned(),
        ));
    }
    serde_json::from_reader(file).map_err(JobError::from)
}

fn valid_leaf_name(value: &str) -> bool {
    !value.is_empty() && Path::new(value).file_name().and_then(|name| name.to_str()) == Some(value)
}

fn refuse_existing(path: &Path) -> Result<(), JobError> {
    match path.symlink_metadata() {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(JobError::Integrity(format!(
            "publication destination already exists: {}",
            path.display()
        ))),
        Err(source) => Err(io_error(path, source)),
    }
}

fn rename_noclobber(
    source_directory: &File,
    source_name: &Path,
    destination_directory: &File,
    destination_name: &Path,
    destination: &Path,
) -> Result<(), JobError> {
    use rustix::fs::{RenameFlags, renameat_with};
    use rustix::io::Errno;

    match renameat_with(
        source_directory,
        source_name,
        destination_directory,
        destination_name,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(JobError::Integrity(format!(
            "publication destination already exists: {}",
            destination.display()
        ))),
        Err(Errno::NOSYS | Errno::INVAL | Errno::NOTSUP) => Err(io_error(
            destination,
            std::io::Error::new(
                ErrorKind::Unsupported,
                "atomic no-replace rename is unsupported by this kernel or filesystem",
            ),
        )),
        Err(source) => Err(io_error(destination, source.into())),
    }
}

fn replace_at(
    source_directory: &File,
    source_name: &Path,
    destination_directory: &File,
    destination_name: &Path,
    destination: &Path,
) -> Result<(), JobError> {
    rustix::fs::renameat(
        source_directory,
        source_name,
        destination_directory,
        destination_name,
    )
    .map_err(|source| io_error(destination, source.into()))
}

fn verify_recovery_log(path: &Path, expected: &str) -> Result<(), JobError> {
    let metadata = path
        .symlink_metadata()
        .map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink()
        || !private_state_attributes_valid(
            metadata.is_file(),
            metadata.uid(),
            metadata.mode(),
            Some(metadata.nlink()),
        )
        || metadata.len()
            != u64::try_from(expected.len())
                .map_err(|_| JobError::Integrity("recovery log length overflow".to_owned()))?
    {
        return Err(JobError::Integrity(
            "published recovery log has unsafe attributes".to_owned(),
        ));
    }
    let actual = fs::read_to_string(path).map_err(|source| io_error(path, source))?;
    if actual != expected {
        return Err(JobError::Integrity(
            "published recovery log content mismatch".to_owned(),
        ));
    }
    Ok(())
}

fn require_message(active: Option<&ActiveCandidate>, message_id: u32) -> Result<(), JobError> {
    match active {
        Some(active) if active.message_id == message_id => Ok(()),
        Some(_) => Err(JobError::EventSequence(
            "event message identifier does not match active message".to_owned(),
        )),
        None => Err(JobError::EventSequence(
            "message event occurred without an active message".to_owned(),
        )),
    }
}

fn validate_property_owner(
    active: Option<&ActiveCandidate>,
    attachment: Option<&ActiveAttachment>,
    owner: PropertyOwner,
) -> Result<(), JobError> {
    match owner {
        PropertyOwner::Folder(_) if active.is_none() => Ok(()),
        PropertyOwner::Folder(_) => Err(JobError::EventSequence(
            "folder property occurred during a message".to_owned(),
        )),
        PropertyOwner::Message(message_id) => require_message(active, message_id),
        PropertyOwner::Recipient { message_id, index } => {
            require_message(active, message_id)?;
            if active.is_some_and(|candidate| candidate.recipients.contains(&index)) {
                Ok(())
            } else {
                Err(JobError::EventSequence(
                    "recipient property does not match an emitted recipient".to_owned(),
                ))
            }
        }
        PropertyOwner::Attachment { message_id, index } => {
            require_message(active, message_id)?;
            match attachment {
                Some(active) if active.message_id == message_id && active.index == index => Ok(()),
                _ => Err(JobError::EventSequence(
                    "attachment property does not match active attachment".to_owned(),
                )),
            }
        }
    }
}

fn digest_hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut value, "{byte:02x}");
    }
    value
}

fn set_mode(path: &Path, mode: u32) -> Result<(), JobError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|source| io_error(path, source))
}

fn validate_private_directory(directory: &File, path: &Path) -> Result<(), JobError> {
    let metadata = directory
        .metadata()
        .map_err(|source| io_error(path, source))?;
    if !private_state_attributes_valid(metadata.is_dir(), metadata.uid(), metadata.mode(), None) {
        return Err(JobError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

fn validate_private_file(path: &Path, required: bool) -> Result<(), JobError> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if !required && error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error(path, source)),
    };
    if metadata.file_type().is_symlink()
        || !private_state_attributes_valid(
            metadata.is_file(),
            metadata.uid(),
            metadata.mode(),
            Some(metadata.nlink()),
        )
    {
        return Err(JobError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

fn private_state_attributes_valid(
    kind_matches: bool,
    uid: u32,
    mode: u32,
    nlink: Option<u64>,
) -> bool {
    kind_matches
        && uid == rustix::process::geteuid().as_raw()
        && mode & 0o077 == 0
        && nlink.is_none_or(|links| links == 1)
}

fn secure_sqlite_files(private_root: &Path) -> Result<(), JobError> {
    for name in ["job.sqlite3", "job.sqlite3-wal", "job.sqlite3-shm"] {
        let path = private_root.join(name);
        match path.symlink_metadata() {
            Ok(_) => set_mode(&path, 0o600)?,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(&path, source)),
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), JobError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error(path, source))
}

fn sync_file(file: &File, path: &Path) -> Result<(), JobError> {
    file.sync_all().map_err(|source| io_error(path, source))
}

fn open_directory(path: &Path) -> Result<File, JobError> {
    let initial = path
        .symlink_metadata()
        .map_err(|source| io_error(path, source))?;
    if initial.file_type().is_symlink() || !initial.is_dir() {
        return Err(JobError::UnsafePath(path.to_path_buf()));
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    let opened = directory
        .metadata()
        .map_err(|source| io_error(path, source))?;
    if initial.dev() != opened.dev() || initial.ino() != opened.ino() || !opened.is_dir() {
        return Err(JobError::UnsafePath(path.to_path_buf()));
    }
    Ok(directory)
}

fn fd_path(fd: RawFd) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{fd}"))
}

fn job_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn validate_foreign_keys(connection: &Connection) -> Result<(), JobError> {
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    if statement.query([])?.next()?.is_some() {
        return Err(JobError::Integrity(
            "foreign-key validation failed".to_owned(),
        ));
    }
    let invalid = connection.query_row(
        r#"SELECT EXISTS(
            SELECT 1 FROM candidates WHERE status = 'pending'
            UNION ALL
            SELECT 1 FROM candidate_events
            WHERE (blob_sha256 IS NULL) != (byte_len IS NULL)
            UNION ALL
            SELECT 1 FROM candidate_events e
            JOIN blobs b ON b.sha256 = e.blob_sha256
            WHERE e.byte_len IS NOT b.byte_len
            UNION ALL
            SELECT 1 FROM blobs b WHERE NOT EXISTS(
                SELECT 1 FROM candidate_events e WHERE e.blob_sha256 = b.sha256
            )
            UNION ALL
            SELECT 1 FROM candidates c
            WHERE c.status = 'written' AND NOT EXISTS(
                SELECT 1 FROM part_items i WHERE i.item_key = c.item_key
            )
            UNION ALL
            SELECT 1 FROM part_items i
            JOIN candidates c ON c.item_key = i.item_key
            WHERE c.status != 'written'
            UNION ALL
            SELECT 1 FROM parts p WHERE NOT EXISTS(
                SELECT 1 FROM part_items i WHERE i.part_index = p.part_index
            )
            UNION ALL
            SELECT 1 FROM parts p
            WHERE p.oversize = 1 AND (
                SELECT COUNT(*) FROM part_items i
                JOIN candidates c ON c.item_key = i.item_key
                WHERE i.part_index = p.part_index AND c.parent_item_key IS NULL
            ) != 1
         )"#,
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if invalid {
        return Err(JobError::Integrity(
            "logical ledger validation failed".to_owned(),
        ));
    }
    Ok(())
}

fn validate_blob_store(
    connection: &Connection,
    spool: &Path,
    interrupted: Option<&AtomicBool>,
) -> Result<(), JobError> {
    let pack_path = spool.join(PAYLOAD_PACK_FILENAME);
    let pack_metadata = pack_path
        .symlink_metadata()
        .map_err(|source| io_error(&pack_path, source))?;
    if pack_metadata.file_type().is_symlink()
        || !private_state_attributes_valid(
            pack_metadata.is_file(),
            pack_metadata.uid(),
            pack_metadata.mode(),
            Some(pack_metadata.nlink()),
        )
    {
        return Err(JobError::Integrity(
            "payload pack has invalid private-file attributes".to_owned(),
        ));
    }
    let mut pack = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&pack_path)
        .map_err(|source| io_error(&pack_path, source))?;
    let opened_pack = pack
        .metadata()
        .map_err(|source| io_error(&pack_path, source))?;
    if opened_pack.dev() != pack_metadata.dev()
        || opened_pack.ino() != pack_metadata.ino()
        || opened_pack.len() != pack_metadata.len()
    {
        return Err(JobError::Integrity(
            "payload pack changed while opening".to_owned(),
        ));
    }
    let mut statement = connection.prepare(
        "SELECT DISTINCT b.sha256, b.byte_len, b.pack_offset, length(i.data), \
                CASE WHEN length(i.data) <= ?1 THEN i.data END \
         FROM blobs b \
         JOIN candidate_events e ON e.blob_sha256 = b.sha256 \
         JOIN candidates c ON c.item_key = e.item_key \
         LEFT JOIN inline_blobs i ON i.sha256 = b.sha256 \
         WHERE c.status IN ('pending', 'spooled', 'failed') \
         ORDER BY b.pack_offset IS NULL, b.pack_offset, b.sha256",
    )?;
    let mut rows = statement.query([INLINE_BLOB_MAX_BYTES])?;
    while let Some(row) = rows.next()? {
        let sha256 = row.get::<_, String>(0)?;
        let byte_len = row.get::<_, u64>(1)?;
        let pack_offset = row.get::<_, Option<u64>>(2)?;
        let inline_len = row.get::<_, Option<u64>>(3)?;
        let inline = row.get::<_, Option<Vec<u8>>>(4)?;
        if !valid_blob_hash(&sha256) {
            return Err(JobError::Integrity(
                "blob key is not lowercase SHA-256".to_owned(),
            ));
        }
        check_interrupted(interrupted)?;
        let legacy_path = spool.join(&sha256);
        let legacy_exists = legacy_path
            .try_exists()
            .map_err(|source| io_error(&legacy_path, source))?;
        if let Some(offset) = pack_offset {
            if inline_len.is_some() || inline.is_some() || legacy_exists {
                return Err(JobError::Integrity(format!(
                    "blob {sha256} has multiple storage representations"
                )));
            }
            verify_pack_range(
                &mut pack,
                &pack_path,
                pack_metadata.len(),
                offset,
                byte_len,
                &sha256,
                interrupted,
            )?;
        } else if let Some(inline_len) = inline_len {
            if inline_len != byte_len || inline_len > INLINE_BLOB_MAX_BYTES {
                return Err(JobError::Integrity(format!(
                    "inline blob {sha256} has an invalid bounded length"
                )));
            }
            if legacy_exists {
                return Err(JobError::Integrity(format!(
                    "blob {sha256} has multiple storage representations"
                )));
            }
            let data = inline.ok_or_else(|| {
                JobError::Integrity(format!("inline blob {sha256} payload is unavailable"))
            })?;
            verify_blob_bytes(&sha256, byte_len, &data)?;
        } else {
            if inline.is_some() {
                return Err(JobError::Integrity(format!(
                    "file blob {sha256} has unexpected inline data"
                )));
            }
            open_verified_blob_with_interrupt(&legacy_path, &sha256, byte_len, interrupted)?;
        }
    }
    Ok(())
}

fn verify_pack_range(
    pack: &mut File,
    path: &Path,
    pack_len: u64,
    offset: u64,
    byte_len: u64,
    expected_hash: &str,
    interrupted: Option<&AtomicBool>,
) -> Result<(), JobError> {
    let end = offset
        .checked_add(byte_len)
        .ok_or_else(|| JobError::Integrity("payload pack range overflow".to_owned()))?;
    if end > pack_len {
        return Err(JobError::Integrity(format!(
            "payload pack range for {expected_hash} exceeds the durable file"
        )));
    }
    pack.seek(std::io::SeekFrom::Start(offset))
        .map_err(|source| io_error(path, source))?;
    let mut remaining = byte_len;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        check_interrupted(interrupted)?;
        let limit = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| JobError::Integrity("payload pack read length overflow".to_owned()))?;
        let read = pack
            .read(&mut buffer[..limit])
            .map_err(|source| io_error(path, source))?;
        if read == 0 {
            return Err(JobError::Integrity(format!(
                "payload pack range for {expected_hash} ended early"
            )));
        }
        hasher.update(&buffer[..read]);
        remaining =
            remaining
                .checked_sub(u64::try_from(read).map_err(|_| {
                    JobError::Integrity("payload pack read length overflow".to_owned())
                })?)
                .ok_or_else(|| JobError::Integrity("payload pack read underflow".to_owned()))?;
    }
    if digest_hex(hasher.finalize().as_slice()) != expected_hash {
        return Err(JobError::Integrity(format!(
            "payload pack range for {expected_hash} failed SHA-256 validation"
        )));
    }
    Ok(())
}

fn validate_part_store(
    connection: &Connection,
    parts: &Path,
    manifests: &Path,
    interrupted: Option<&AtomicBool>,
) -> Result<(), JobError> {
    let mut statement = connection.prepare(
        "SELECT part_index, filename, byte_len, sha256, oversize, sidecar_json \
         FROM parts ORDER BY part_index",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                PublishedPart {
                    index: row.get(0)?,
                    filename: row.get(1)?,
                    byte_len: row.get(2)?,
                    sha256: row.get(3)?,
                    oversize: row.get(4)?,
                },
                row.get::<_, String>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut accounted_sidecars = HashSet::new();
    for (part, sidecar_json) in rows {
        validate_part_record(&part)?;
        let sidecar_filename = part.filename.trim_end_matches(".pst").to_owned() + ".json";
        let expected_sidecar: PartSidecar = serde_json::from_str(&sidecar_json)?;
        validate_sidecar(&part, &expected_sidecar)?;
        verify_part_artifact(
            &parts.join(&part.filename),
            &part,
            &expected_sidecar,
            interrupted,
        )?;
        verify_sidecar_artifact(&manifests.join(&sidecar_filename), &expected_sidecar)?;
        accounted_sidecars.insert(sidecar_filename);
    }
    for entry in fs::read_dir(manifests).map_err(|source| io_error(manifests, source))? {
        let entry = entry.map_err(|source| io_error(manifests, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".json") && !accounted_sidecars.contains(name.as_ref()) {
            return Err(JobError::Integrity(format!(
                "finalized sidecar {name:?} has no ledger record"
            )));
        } else if !accounted_sidecars.contains(name.as_ref()) {
            return Err(JobError::Integrity(format!(
                "unrecognized private manifest entry {name:?}"
            )));
        }
    }
    Ok(())
}

fn verify_part_artifact(
    path: &Path,
    part: &PublishedPart,
    sidecar: &PartSidecar,
    interrupted: Option<&AtomicBool>,
) -> Result<(), JobError> {
    let metadata = path
        .symlink_metadata()
        .map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink()
        || !private_state_attributes_valid(
            metadata.is_file(),
            metadata.uid(),
            metadata.mode(),
            Some(metadata.nlink()),
        )
        || metadata.len() != part.byte_len
    {
        return Err(JobError::Integrity(format!(
            "published part {} has an invalid type or size",
            part.filename
        )));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    let opened = file.metadata().map_err(|source| io_error(path, source))?;
    if metadata.dev() != opened.dev()
        || metadata.ino() != opened.ino()
        || !private_state_attributes_valid(
            opened.is_file(),
            opened.uid(),
            opened.mode(),
            Some(opened.nlink()),
        )
        || opened.len() != part.byte_len
        || sidecar
            .published_device
            .is_some_and(|expected| expected != opened.dev())
        || sidecar
            .published_inode
            .is_some_and(|expected| expected != opened.ino())
    {
        return Err(JobError::Integrity(format!(
            "published part {} changed while opening",
            part.filename
        )));
    }
    let Some(expected) = part.sha256.as_deref() else {
        return Ok(());
    };
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if interrupted.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(JobError::Interrupted);
        }
        let read = file
            .read(&mut buffer)
            .map_err(|source| io_error(path, source))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if digest_hex(hasher.finalize().as_slice()) != expected {
        return Err(JobError::Integrity(format!(
            "published part {} failed SHA-256 validation",
            part.filename
        )));
    }
    Ok(())
}

fn check_interrupted(interrupted: Option<&AtomicBool>) -> Result<(), JobError> {
    if interrupted.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        Err(JobError::Interrupted)
    } else {
        Ok(())
    }
}

fn valid_blob_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn verify_blob(path: &Path, expected_hash: &str, expected_len: u64) -> Result<(), JobError> {
    open_verified_blob_with_interrupt(path, expected_hash, expected_len, None).map(|_| ())
}

fn open_verified_blob_with_interrupt(
    path: &Path,
    expected_hash: &str,
    expected_len: u64,
    interrupted: Option<&AtomicBool>,
) -> Result<File, JobError> {
    let metadata = path
        .symlink_metadata()
        .map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink()
        || !private_state_attributes_valid(
            metadata.is_file(),
            metadata.uid(),
            metadata.mode(),
            Some(metadata.nlink()),
        )
        || metadata.len() != expected_len
    {
        return Err(JobError::Integrity(format!(
            "blob {expected_hash} has an invalid type or size"
        )));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    let opened = file.metadata().map_err(|source| io_error(path, source))?;
    if metadata.dev() != opened.dev()
        || metadata.ino() != opened.ino()
        || !opened.is_file()
        || opened.uid() != rustix::process::geteuid().as_raw()
        || opened.mode() & 0o077 != 0
        || opened.nlink() != 1
        || opened.len() != expected_len
    {
        return Err(JobError::Integrity(format!(
            "blob {expected_hash} changed while opening"
        )));
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if interrupted.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(JobError::Interrupted);
        }
        let read = file
            .read(&mut buffer)
            .map_err(|source| io_error(path, source))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = digest_hex(hasher.finalize().as_slice());
    if actual != expected_hash {
        return Err(JobError::Integrity(format!(
            "blob {expected_hash} failed SHA-256 validation"
        )));
    }
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|source| io_error(path, source))?;
    Ok(file)
}

fn check_job_interrupted(interrupted: Option<&AtomicBool>) -> Result<(), JobError> {
    if interrupted.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        Err(JobError::Interrupted)
    } else {
        Ok(())
    }
}

fn remove_temporary_blobs(spool: &Path, interrupted: Option<&AtomicBool>) -> Result<(), JobError> {
    for entry in fs::read_dir(spool).map_err(|source| io_error(spool, source))? {
        check_job_interrupted(interrupted)?;
        let entry = entry.map_err(|source| io_error(spool, source))?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".tmp") {
            fs::remove_file(entry.path()).map_err(|source| io_error(&entry.path(), source))?;
        }
    }
    Ok(())
}

fn remove_stale_partials(
    directory: &File,
    partial: &Path,
    interrupted: Option<&AtomicBool>,
) -> Result<(), JobError> {
    let mut removed = false;
    for entry in fs::read_dir(partial).map_err(|source| io_error(partial, source))? {
        check_job_interrupted(interrupted)?;
        let entry = entry.map_err(|source| io_error(partial, source))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| JobError::UnsafePath(entry.path()))?;
        let stat = rustix::fs::statat(
            directory,
            Path::new(&name),
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|source| io_error(&entry.path(), source.into()))?;
        match rustix::fs::FileType::from_raw_mode(stat.st_mode) {
            rustix::fs::FileType::RegularFile
                if valid_leaf_name(&name) && name.ends_with(".partial") =>
            {
                validate_private_stat(&entry.path(), &stat)?;
                rustix::fs::unlinkat(directory, Path::new(&name), rustix::fs::AtFlags::empty())
                    .map_err(|source| io_error(&entry.path(), source.into()))?;
                removed = true;
            }
            rustix::fs::FileType::Directory if name.starts_with(".pstforge-") => {
                if !private_state_attributes_valid(true, stat.st_uid, stat.st_mode, None) {
                    return Err(JobError::UnsafePath(entry.path()));
                }
                removed |= remove_writer_scratch(directory, partial, &name, interrupted)?;
            }
            _ => return Err(JobError::UnsafePath(entry.path())),
        }
    }
    if removed {
        sync_file(directory, partial)?;
    }
    Ok(())
}

fn remove_writer_scratch(
    parent: &File,
    partial: &Path,
    name: &str,
    interrupted: Option<&AtomicBool>,
) -> Result<bool, JobError> {
    let owned = rustix::fs::openat(
        parent,
        Path::new(name),
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|source| io_error(&partial.join(name), source.into()))?;
    let directory = File::from(owned);
    let path = fd_path(directory.as_raw_fd());
    validate_private_directory(&directory, &path)?;
    let mut temporary_names = Vec::new();
    let mut retain_diagnostic = false;
    for entry in fs::read_dir(&path).map_err(|source| io_error(&path, source))? {
        check_job_interrupted(interrupted)?;
        let entry = entry.map_err(|source| io_error(&path, source))?;
        let child_name = entry
            .file_name()
            .into_string()
            .map_err(|_| JobError::UnsafePath(entry.path()))?;
        let stat = rustix::fs::statat(
            &directory,
            Path::new(&child_name),
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|source| io_error(&entry.path(), source.into()))?;
        if child_name == "validator-failure.log" {
            retain_diagnostic = true;
        } else if child_name.starts_with(".readpst-")
            && rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::Directory
        {
            remove_private_tree(
                &directory,
                Path::new(&child_name),
                &path.join(&child_name),
                interrupted,
                0,
            )?;
            continue;
        } else if !child_name.starts_with(".tmp") {
            return Err(JobError::UnsafePath(entry.path()));
        }
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile {
            return Err(JobError::UnsafePath(entry.path()));
        }
        validate_private_stat(&entry.path(), &stat)?;
        temporary_names.push(child_name);
    }
    if retain_diagnostic {
        return Ok(false);
    }
    for child_name in temporary_names {
        check_job_interrupted(interrupted)?;
        rustix::fs::unlinkat(
            &directory,
            Path::new(&child_name),
            rustix::fs::AtFlags::empty(),
        )
        .map_err(|source| io_error(&path.join(&child_name), source.into()))?;
    }
    sync_file(&directory, &path)?;
    rustix::fs::unlinkat(parent, Path::new(name), rustix::fs::AtFlags::REMOVEDIR)
        .map_err(|source| io_error(&partial.join(name), source.into()))?;
    Ok(true)
}

fn remove_private_tree(
    parent: &File,
    name: &Path,
    logical_path: &Path,
    interrupted: Option<&AtomicBool>,
    depth: u32,
) -> Result<(), JobError> {
    if depth >= 64 {
        return Err(JobError::UnsafePath(logical_path.to_path_buf()));
    }
    let owned = rustix::fs::openat(
        parent,
        name,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|source| io_error(logical_path, source.into()))?;
    let directory = File::from(owned);
    let path = fd_path(directory.as_raw_fd());
    for entry in fs::read_dir(&path).map_err(|source| io_error(&path, source))? {
        check_job_interrupted(interrupted)?;
        let entry = entry.map_err(|source| io_error(&path, source))?;
        let child_name = entry.file_name();
        let child_path = logical_path.join(&child_name);
        let stat = rustix::fs::statat(
            &directory,
            Path::new(&child_name),
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|source| io_error(&child_path, source.into()))?;
        match rustix::fs::FileType::from_raw_mode(stat.st_mode) {
            rustix::fs::FileType::RegularFile
                if stat.st_uid == rustix::process::geteuid().as_raw() && stat.st_nlink == 1 =>
            {
                rustix::fs::unlinkat(
                    &directory,
                    Path::new(&child_name),
                    rustix::fs::AtFlags::empty(),
                )
                .map_err(|source| io_error(&child_path, source.into()))?;
            }
            rustix::fs::FileType::Directory
                if stat.st_uid == rustix::process::geteuid().as_raw() =>
            {
                remove_private_tree(
                    &directory,
                    Path::new(&child_name),
                    &child_path,
                    interrupted,
                    depth + 1,
                )?;
            }
            _ => return Err(JobError::UnsafePath(child_path)),
        }
    }
    sync_file(&directory, &path)?;
    rustix::fs::unlinkat(parent, name, rustix::fs::AtFlags::REMOVEDIR)
        .map_err(|source| io_error(logical_path, source.into()))?;
    Ok(())
}

fn validate_private_stat(path: &Path, stat: &rustix::fs::Stat) -> Result<(), JobError> {
    if !private_state_attributes_valid(true, stat.st_uid, stat.st_mode, Some(stat.st_nlink)) {
        return Err(JobError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

fn remove_spool_contents(
    directory: &File,
    spool: &Path,
    interrupted: Option<&AtomicBool>,
) -> Result<(), JobError> {
    remove_private_regular_files(
        directory,
        spool,
        |name| valid_blob_hash(name) || name.starts_with(".tmp") || name == PAYLOAD_PACK_FILENAME,
        interrupted,
    )
}

fn remove_private_regular_files(
    directory: &File,
    path: &Path,
    accepted_name: impl Fn(&str) -> bool,
    interrupted: Option<&AtomicBool>,
) -> Result<(), JobError> {
    let mut removed = false;
    for entry in fs::read_dir(path).map_err(|source| io_error(path, source))? {
        check_job_interrupted(interrupted)?;
        let entry = entry.map_err(|source| io_error(path, source))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| JobError::UnsafePath(entry.path()))?;
        if !accepted_name(&name) {
            return Err(JobError::UnsafePath(entry.path()));
        }
        let stat = rustix::fs::statat(
            directory,
            Path::new(&name),
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|source| io_error(&entry.path(), source.into()))?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
            || !private_state_attributes_valid(true, stat.st_uid, stat.st_mode, Some(stat.st_nlink))
        {
            return Err(JobError::UnsafePath(entry.path()));
        }
        rustix::fs::unlinkat(directory, Path::new(&name), rustix::fs::AtFlags::empty())
            .map_err(|source| io_error(&entry.path(), source.into()))?;
        removed = true;
    }
    if removed {
        sync_file(directory, path)?;
    }
    Ok(())
}

fn remove_unreferenced_blobs(
    connection: &Connection,
    spool: &Path,
    interrupted: Option<&AtomicBool>,
) -> Result<(), JobError> {
    for entry in fs::read_dir(spool).map_err(|source| io_error(spool, source))? {
        check_job_interrupted(interrupted)?;
        let entry = entry.map_err(|source| io_error(spool, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == PAYLOAD_PACK_FILENAME {
            continue;
        }
        if !valid_blob_hash(&name) {
            return Err(JobError::UnsafePath(entry.path()));
        }
        let referenced = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM blobs WHERE sha256 = ?1)",
            [name.as_ref()],
            |row| row.get::<_, bool>(0),
        )?;
        if !referenced {
            fs::remove_file(entry.path()).map_err(|source| io_error(&entry.path(), source))?;
        }
    }
    Ok(())
}

fn io_error(path: &Path, source: std::io::Error) -> JobError {
    JobError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write as _;
    use std::io::{BufRead, BufReader, Read as _};
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    use libpff_sys::{
        CatalogEvent, CatalogProvenance, CatalogSink, NamedPropertyIdentity, NamedPropertyName,
        PayloadRequest, PropertyDescriptor, PropertyOwner,
    };
    use rusqlite::Connection;
    use serde_json::json;
    use tempfile::tempdir;

    use sha2::{Digest, Sha256};

    use super::{
        CANDIDATE_CHECKPOINT_BATCH, CandidateRejectionCategory, DIRECT_SQLITE_CACHE_KIB,
        DirectEmbeddedCandidate, DurableCatalogSink, INLINE_BLOB_MAX_BYTES, INLINE_CACHE_DIRECTORY,
        JobConfiguration, JobError, JobSourceIdentity, PAYLOAD_PACK_FILENAME, PartSidecar,
        PayloadPackMetrics, PublishedPart, WorkerEvent, digest_hex, private_state_attributes_valid,
        read_validated_report_snapshot, run_sql_interruptible, validate_blob_store, write_hashed,
        write_sidecar_partial,
    };

    struct PrefixThenError {
        remaining: usize,
    }

    #[test]
    fn report_snapshot_read_is_validated_and_does_not_mutate_the_job()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.bind_source(&JobSourceIdentity {
            canonical_path: "/external/mail.pst".to_owned(),
            device: 1,
            inode: 2,
            size_bytes: 3,
            modified_at: "2026-07-20T00:00:00Z".to_owned(),
            sha256: Some("a".repeat(64)),
        })?;
        sink.bind_configuration(&JobConfiguration {
            tool_compatibility_major: 0,
            split_schema_version: "0.5.0".to_owned(),
            execution_mode: "direct".to_owned(),
            recovery_mode: "balanced".to_owned(),
            maximum_pst_bytes: 4_294_967_296,
            part_size_policy: "hard-maximum-v1".to_owned(),
            writer_format: "unicode-pst-v23".to_owned(),
        })?;
        let snapshot = json!({"schema_version": "0.5.0", "parts": []});
        sink.publish_report_snapshot(&snapshot)?;
        sink.checkpoint()?;
        drop(sink);

        let database = job.join(".pstforge/job.sqlite3");
        let before = fs::read(&database)?;
        let private_entries = |path: &Path| -> Result<Vec<String>, std::io::Error> {
            let mut entries = fs::read_dir(path)?
                .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
                .collect::<Result<Vec<_>, std::io::Error>>()?;
            entries.sort();
            Ok(entries)
        };
        let private_entries_before = private_entries(&job.join(".pstforge"))?;
        let validated = read_validated_report_snapshot(&job)?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&validated.json)?,
            snapshot
        );
        assert!(validated.parts.is_empty());
        assert_eq!(fs::read(&database)?, before);
        assert_eq!(
            private_entries(&job.join(".pstforge"))?,
            private_entries_before
        );

        let connection = Connection::open(&database)?;
        connection.execute(
            "INSERT INTO job_metadata(key, value) VALUES ('worker_attempts', '1')",
            [],
        )?;
        drop(connection);
        assert!(matches!(
            read_validated_report_snapshot(&job),
            Err(JobError::Integrity(_))
        ));
        let connection = Connection::open(&database)?;
        connection.execute("DELETE FROM job_metadata WHERE key = 'worker_attempts'", [])?;
        connection.execute(
            "UPDATE job_metadata SET value = value || ' ' WHERE key = 'split_report_snapshot'",
            [],
        )?;
        drop(connection);
        assert!(matches!(
            read_validated_report_snapshot(&job),
            Err(JobError::Integrity(_))
        ));
        Ok(())
    }

    #[test]
    fn resume_invalidates_a_completed_report_before_changing_job_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        let source = JobSourceIdentity {
            canonical_path: "/external/mail.pst".to_owned(),
            device: 1,
            inode: 2,
            size_bytes: 3,
            modified_at: "2026-07-20T00:00:00Z".to_owned(),
            sha256: Some("a".repeat(64)),
        };
        let configuration = JobConfiguration {
            tool_compatibility_major: 0,
            split_schema_version: "0.5.0".to_owned(),
            execution_mode: "restartable".to_owned(),
            recovery_mode: "balanced".to_owned(),
            maximum_pst_bytes: 4_294_967_296,
            part_size_policy: "hard-maximum-v1".to_owned(),
            writer_format: "unicode-pst-v23".to_owned(),
        };
        sink.bind_source(&source)?;
        sink.bind_recovery_mode("balanced")?;
        sink.bind_configuration(&configuration)?;
        sink.publish_report_snapshot(&json!({"schema_version": "0.5.0"}))?;
        sink.checkpoint()?;
        drop(sink);
        assert!(read_validated_report_snapshot(&job).is_ok());

        let resumed = DurableCatalogSink::open_resume(&job, &source, &configuration)?;
        drop(resumed);
        assert!(matches!(
            read_validated_report_snapshot(&job),
            Err(JobError::ReportUnavailable(_))
        ));
        Ok(())
    }

    #[test]
    fn report_explains_jobs_that_predate_snapshot_storage() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.checkpoint()?;
        drop(sink);
        assert!(matches!(
            read_validated_report_snapshot(&job),
            Err(JobError::ReportUnavailable(_))
        ));
        Ok(())
    }

    #[test]
    fn resume_configuration_is_exact_and_mismatch_validation_is_read_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        let source = JobSourceIdentity {
            canonical_path: "/external/mail.pst".to_owned(),
            device: 1,
            inode: 2,
            size_bytes: 3,
            modified_at: "2026-07-16T00:00:00Z".to_owned(),
            sha256: Some("a".repeat(64)),
        };
        let configuration = JobConfiguration {
            tool_compatibility_major: 0,
            split_schema_version: "0.4.4".to_owned(),
            execution_mode: "restartable".to_owned(),
            recovery_mode: "balanced".to_owned(),
            maximum_pst_bytes: 4_294_967_296,
            part_size_policy: "hard-maximum-v1".to_owned(),
            writer_format: "unicode-pst-v23".to_owned(),
        };
        sink.bind_source(&source)?;
        sink.bind_recovery_mode("balanced")?;
        sink.bind_configuration(&configuration)?;
        sink.checkpoint()?;
        drop(sink);

        DurableCatalogSink::validate_resume(&job, &source, &configuration)?;
        let resumed = DurableCatalogSink::open_resume(&job, &source, &configuration)?;
        assert!(resumed.allocated_bytes()? > 0);
        drop(resumed);
        let database = job.join(".pstforge/job.sqlite3");
        let before = Sha256::digest(std::fs::read(&database)?);
        let mut mismatch = configuration.clone();
        mismatch.maximum_pst_bytes -= 1;
        assert!(matches!(
            DurableCatalogSink::validate_resume(&job, &source, &mismatch),
            Err(JobError::ResumeMismatch("part-size policy"))
        ));
        assert!(matches!(
            DurableCatalogSink::open_resume(&job, &source, &mismatch),
            Err(JobError::ResumeMismatch("part-size policy"))
        ));
        let mut mismatch = configuration.clone();
        mismatch.execution_mode = "direct".to_owned();
        assert!(matches!(
            DurableCatalogSink::validate_resume(&job, &source, &mismatch),
            Err(JobError::ResumeMismatch("execution mode"))
        ));
        let mut wrong_source = source.clone();
        wrong_source.sha256 = Some("b".repeat(64));
        assert!(matches!(
            DurableCatalogSink::validate_resume(&job, &wrong_source, &configuration),
            Err(JobError::ResumeMismatch("source identity or SHA-256"))
        ));
        let after = Sha256::digest(std::fs::read(&database)?);
        assert_eq!(before, after);
        Ok(())
    }

    #[test]
    fn legacy_job_configuration_defaults_to_restartable() -> Result<(), serde_json::Error> {
        let configuration: JobConfiguration = serde_json::from_value(serde_json::json!({
            "tool_compatibility_major": 0,
            "split_schema_version": "0.4.4",
            "recovery_mode": "balanced",
            "maximum_pst_bytes": 4_294_967_296_u64,
            "part_size_policy": "hard-maximum-v1",
            "writer_format": "unicode-pst-v23"
        }))?;
        assert_eq!(configuration.execution_mode, "restartable");
        Ok(())
    }

    #[test]
    fn resume_refuses_invalid_rejection_state_before_mutating_work()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        let source = JobSourceIdentity {
            canonical_path: "/external/mail.pst".to_owned(),
            device: 1,
            inode: 2,
            size_bytes: 3,
            modified_at: "2026-07-16T00:00:00Z".to_owned(),
            sha256: Some("a".repeat(64)),
        };
        let configuration = JobConfiguration {
            tool_compatibility_major: 0,
            split_schema_version: "0.4.4".to_owned(),
            execution_mode: "restartable".to_owned(),
            recovery_mode: "balanced".to_owned(),
            maximum_pst_bytes: 4_294_967_296,
            part_size_policy: "hard-maximum-v1".to_owned(),
            writer_format: "unicode-pst-v23".to_owned(),
        };
        sink.bind_source(&source)?;
        sink.bind_recovery_mode("balanced")?;
        sink.bind_configuration(&configuration)?;
        message_start(&mut sink, 10)?;
        let descriptor = body_descriptor(10, 4);
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: b"body",
        })?;
        sink.event(CatalogEvent::PropertyEnd(descriptor))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.mark_candidates_unsupported(
            &["normal:10:-:0".to_owned()],
            CandidateRejectionCategory::MalformedProperty,
        )?;
        sink.checkpoint()?;
        drop(sink);

        let database = job.join(".pstforge/job.sqlite3");
        let connection = Connection::open(&database)?;
        connection.execute(
            "UPDATE candidate_events SET metadata_json = '{}' \
             WHERE kind = 'output_unrepresentable'",
            [],
        )?;
        drop(connection);
        let payload_path = job.join(".pstforge/spool").join(PAYLOAD_PACK_FILENAME);
        let payload_before = std::fs::read(&payload_path)?;
        let parts_before = std::fs::read_dir(job.join("parts"))?.count();

        assert!(matches!(
            DurableCatalogSink::validate_resume(&job, &source, &configuration),
            Err(JobError::Integrity(_))
        ));
        assert!(matches!(
            DurableCatalogSink::open_resume(&job, &source, &configuration),
            Err(JobError::Integrity(_))
        ));

        let connection = Connection::open(&database)?;
        let state_after = connection.query_row(
            "SELECT candidates.status, candidate_events.metadata_json \
             FROM candidates JOIN candidate_events USING(item_key) \
             WHERE candidate_events.kind = 'output_unrepresentable'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        assert_eq!(state_after, ("unsupported".to_owned(), "{}".to_owned()));
        assert_eq!(std::fs::read(&payload_path)?, payload_before);
        assert_eq!(std::fs::read_dir(job.join("parts"))?.count(), parts_before);

        connection.execute(
            "UPDATE candidate_events \
             SET metadata_json = '{\"schema_version\":1,\"category\":\"malformed_property\"}' \
             WHERE kind = 'output_unrepresentable'",
            [],
        )?;
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        connection.execute(
            "INSERT INTO candidate_events(\
                item_key, sequence, kind, metadata_json, blob_sha256, byte_len\
             ) VALUES (\
                'normal:999:-:0', 1, 'output_unrepresentable',\
                '{\"schema_version\":1,\"category\":\"malformed_candidate\"}', NULL, NULL\
             )",
            [],
        )?;
        drop(connection);
        let database_before = Sha256::digest(std::fs::read(&database)?);
        let payload_before = std::fs::read(&payload_path)?;
        let directory_entries = |path: &Path| -> Result<Vec<String>, std::io::Error> {
            let mut names = std::fs::read_dir(path)?
                .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
                .collect::<Result<Vec<_>, _>>()?;
            names.sort();
            Ok(names)
        };
        let partial_before = directory_entries(&job.join(".pstforge/partial"))?;
        let manifests_before = directory_entries(&job.join(".pstforge/manifests"))?;
        let parts_before = directory_entries(&job.join("parts"))?;

        assert!(matches!(
            DurableCatalogSink::validate_resume(&job, &source, &configuration),
            Err(JobError::Integrity(_))
        ));
        assert!(matches!(
            DurableCatalogSink::open_resume(&job, &source, &configuration),
            Err(JobError::Integrity(_))
        ));
        assert_eq!(Sha256::digest(std::fs::read(&database)?), database_before);
        assert_eq!(std::fs::read(payload_path)?, payload_before);
        assert_eq!(
            directory_entries(&job.join(".pstforge/partial"))?,
            partial_before
        );
        assert_eq!(
            directory_entries(&job.join(".pstforge/manifests"))?,
            manifests_before
        );
        assert_eq!(directory_entries(&job.join("parts"))?, parts_before);
        Ok(())
    }

    #[test]
    fn resume_rejects_schema_twelve_without_calendar_exception_classification()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        let source = JobSourceIdentity {
            canonical_path: "/external/mail.pst".to_owned(),
            device: 1,
            inode: 2,
            size_bytes: 3,
            modified_at: "2026-07-16T00:00:00Z".to_owned(),
            sha256: Some("a".repeat(64)),
        };
        let configuration = JobConfiguration {
            tool_compatibility_major: 0,
            split_schema_version: "0.4.4".to_owned(),
            execution_mode: "restartable".to_owned(),
            recovery_mode: "balanced".to_owned(),
            maximum_pst_bytes: 4_294_967_296,
            part_size_policy: "hard-maximum-v1".to_owned(),
            writer_format: "unicode-pst-v23".to_owned(),
        };
        sink.bind_source(&source)?;
        sink.bind_recovery_mode("balanced")?;
        sink.bind_configuration(&configuration)?;
        sink.checkpoint()?;
        drop(sink);

        let database = job.join(".pstforge/job.sqlite3");
        let connection = Connection::open(&database)?;
        connection.execute(
            "UPDATE job_metadata SET value = '12' WHERE key = 'schema_version'",
            [],
        )?;
        drop(connection);
        assert!(matches!(
            DurableCatalogSink::validate_resume(&job, &source, &configuration),
            Err(JobError::ResumeMismatch("job schema version"))
        ));
        assert!(matches!(
            DurableCatalogSink::open_resume(&job, &source, &configuration),
            Err(JobError::ResumeMismatch("job schema version"))
        ));
        let connection = Connection::open(&database)?;
        connection.execute(
            "UPDATE job_metadata SET value = '15' WHERE key = 'schema_version'",
            [],
        )?;
        drop(connection);
        assert!(matches!(
            DurableCatalogSink::validate_resume(&job, &source, &configuration),
            Err(JobError::ResumeMismatch("job schema version"))
        ));
        assert!(matches!(
            DurableCatalogSink::open_resume(&job, &source, &configuration),
            Err(JobError::ResumeMismatch("job schema version"))
        ));
        let connection = Connection::open(&database)?;
        connection.execute(
            "UPDATE job_metadata SET value = '16' WHERE key = 'schema_version'",
            [],
        )?;
        drop(connection);
        assert!(matches!(
            DurableCatalogSink::validate_resume(&job, &source, &configuration),
            Err(JobError::ResumeMismatch("job schema version"))
        ));
        assert!(matches!(
            DurableCatalogSink::open_resume(&job, &source, &configuration),
            Err(JobError::ResumeMismatch("job schema version"))
        ));
        Ok(())
    }

    #[test]
    fn resume_rejects_schema_fourteen_without_reconstruction_accounting()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        let source = JobSourceIdentity {
            canonical_path: "/external/mail.pst".to_owned(),
            device: 1,
            inode: 2,
            size_bytes: 3,
            modified_at: "2026-07-16T00:00:00Z".to_owned(),
            sha256: Some("a".repeat(64)),
        };
        let configuration = JobConfiguration {
            tool_compatibility_major: 0,
            split_schema_version: "0.4.4".to_owned(),
            execution_mode: "restartable".to_owned(),
            recovery_mode: "balanced".to_owned(),
            maximum_pst_bytes: 4_294_967_296,
            part_size_policy: "hard-maximum-v1".to_owned(),
            writer_format: "unicode-pst-v23".to_owned(),
        };
        sink.bind_source(&source)?;
        sink.bind_recovery_mode("balanced")?;
        sink.bind_configuration(&configuration)?;
        sink.checkpoint()?;
        drop(sink);

        let database = job.join(".pstforge/job.sqlite3");
        let connection = Connection::open(&database)?;
        connection.execute(
            "UPDATE job_metadata SET value = '14' WHERE key = 'schema_version'",
            [],
        )?;
        drop(connection);
        assert!(matches!(
            DurableCatalogSink::validate_resume(&job, &source, &configuration),
            Err(JobError::ResumeMismatch("job schema version"))
        ));
        assert!(matches!(
            DurableCatalogSink::open_resume(&job, &source, &configuration),
            Err(JobError::ResumeMismatch("job schema version"))
        ));
        Ok(())
    }

    #[test]
    fn resume_ignores_untracked_public_output_without_capacity_credit()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.checkpoint()?;
        drop(sink);
        let baseline = DurableCatalogSink::open(&job)?;
        let before = baseline.allocated_bytes()?;
        drop(baseline);
        for name in ["part-0001.log", "part-0001.pst"] {
            let untracked = job.join("parts").join(name);
            std::fs::write(&untracked, vec![0_u8; 4096])?;
            std::fs::set_permissions(&untracked, std::fs::Permissions::from_mode(0o600))?;
        }
        let resumed = DurableCatalogSink::open(&job)?;
        assert_eq!(resumed.allocated_bytes()?, before);
        assert!(matches!(
            resumed.available_part_filename(1),
            Err(JobError::OutputNameConflict(path)) if path.ends_with("part-0001.pst")
        ));
        Ok(())
    }

    #[test]
    fn resume_rejects_untracked_private_storage_before_capacity_credit()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.checkpoint()?;
        drop(sink);
        let untracked = job.join(".pstforge/untracked.bin");
        std::fs::write(&untracked, vec![0_u8; 4096])?;
        std::fs::set_permissions(&untracked, std::fs::Permissions::from_mode(0o600))?;
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::UnsafePath(path)) if path.ends_with("untracked.bin")
        ));
        Ok(())
    }

    #[test]
    fn resume_rejects_untracked_spool_storage_before_capacity_credit()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.checkpoint()?;
        drop(sink);
        let untracked = job.join(".pstforge/spool/untracked.bin");
        std::fs::write(&untracked, vec![0_u8; 4096])?;
        std::fs::set_permissions(&untracked, std::fs::Permissions::from_mode(0o600))?;
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::UnsafePath(path)) if path.ends_with("untracked.bin")
        ));
        Ok(())
    }

    #[test]
    fn resume_capacity_does_not_credit_retained_validator_scratch()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.checkpoint()?;
        let before = sink.allocated_bytes()?;
        let scratch = job.join(".pstforge/partial/.pstforge-validator-failure");
        std::fs::create_dir(&scratch)?;
        std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))?;
        let failed_part = scratch.join(".tmp-retained.pst");
        std::fs::write(&failed_part, vec![0_u8; 1024 * 1024])?;
        std::fs::set_permissions(&failed_part, std::fs::Permissions::from_mode(0o600))?;
        let diagnostic = scratch.join("validator-failure.log");
        std::fs::write(&diagnostic, b"independent validator rejected the part")?;
        std::fs::set_permissions(&diagnostic, std::fs::Permissions::from_mode(0o600))?;
        let failed_allocation = failed_part.metadata()?.blocks().saturating_mul(512);

        let after = sink.allocated_bytes()?;
        assert!(failed_allocation > 0);
        assert!(after.saturating_sub(before) < failed_allocation);
        drop(sink);

        let resumed = DurableCatalogSink::open(&job)?;
        assert!(scratch.exists());
        assert!(resumed.allocated_bytes()?.saturating_sub(before) < failed_allocation);
        Ok(())
    }

    #[test]
    fn reopen_removes_validated_stale_partials_and_refuses_symlinks()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.checkpoint()?;
        drop(sink);
        let stale = job.join(".pstforge/partial/part-0001-attempt-1.pst.partial");
        std::fs::write(&stale, b"incomplete")?;
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o600))?;
        drop(DurableCatalogSink::open(&job)?);
        assert!(!stale.exists());

        let scratch = job.join(".pstforge/partial/.pstforge-stale");
        std::fs::create_dir(&scratch)?;
        std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))?;
        let temporary = scratch.join(".tmp-output");
        std::fs::write(&temporary, b"incomplete")?;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
        drop(DurableCatalogSink::open(&job)?);
        assert!(!scratch.exists());

        let scratch = job.join(".pstforge/partial/.pstforge-readpst-crash");
        let extracted = scratch.join(".readpst-output/Inbox/nested");
        std::fs::create_dir_all(&extracted)?;
        std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))?;
        std::fs::write(extracted.join("message.eml"), b"private recovered mail")?;
        drop(DurableCatalogSink::open(&job)?);
        assert!(!scratch.exists());

        symlink("/dev/null", &stale)?;
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::UnsafePath(_))
        ));
        Ok(())
    }

    impl std::io::Write for PrefixThenError {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::StorageFull));
            }
            let written = self.remaining.min(bytes.len());
            self.remaining -= written;
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn message_start(sink: &mut DurableCatalogSink, id: u32) -> Result<(), String> {
        message_start_with(sink, id, CatalogProvenance::Normal, None, Some(1))
    }

    fn message_start_with(
        sink: &mut DurableCatalogSink,
        id: u32,
        provenance: CatalogProvenance,
        recovery_index: Option<u64>,
        folder_id: Option<u32>,
    ) -> Result<(), String> {
        message_start_with_support(sink, id, provenance, recovery_index, folder_id, true)
    }

    fn message_start_with_support(
        sink: &mut DurableCatalogSink,
        id: u32,
        provenance: CatalogProvenance,
        recovery_index: Option<u64>,
        folder_id: Option<u32>,
        supported: bool,
    ) -> Result<(), String> {
        sink.event(CatalogEvent::MessageStart {
            id,
            provenance,
            recovery_index,
            folder_id,
            parent_message_id: None,
            parent_attachment_index: None,
            embedded_path: Vec::new(),
            associated: false,
            item_type: Some(11),
            message_class: Some("IPM.Note".to_owned()),
            subject: Some("private subject".to_owned()),
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported,
        })
    }

    #[test]
    fn candidate_keys_preserve_recovery_provenance() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        for (id, provenance, recovery_index, folder_id) in [
            (10, CatalogProvenance::Normal, None, Some(1)),
            (20, CatalogProvenance::Recovered, Some(4), None),
            (0, CatalogProvenance::Orphan, Some(7), None),
            (30, CatalogProvenance::Fragment, Some(9), None),
        ] {
            message_start_with(&mut sink, id, provenance, recovery_index, folder_id)?;
            sink.event(CatalogEvent::MessageEnd { id, complete: true })?;
        }
        sink.checkpoint()?;
        let summary = sink.summary()?;
        assert_eq!(summary.committed_candidates, 4);
        assert_eq!(summary.recovered_candidates, 1);
        assert_eq!(summary.orphan_candidates, 1);
        assert_eq!(summary.fragment_candidates, 1);
        drop(sink);
        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        let mut statement = connection.prepare(
            "SELECT item_key, provenance, source_node_id, recovery_index \
             FROM candidates ORDER BY rowid",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<u32>>(2)?,
                    row.get::<_, Option<u64>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            rows,
            vec![
                (
                    "normal:10:-:0".to_owned(),
                    "normal".to_owned(),
                    Some(10),
                    None
                ),
                (
                    "recovered:20:4:0".to_owned(),
                    "recovered".to_owned(),
                    Some(20),
                    Some(4)
                ),
                (
                    "orphan:-:7:0".to_owned(),
                    "orphan".to_owned(),
                    None,
                    Some(7)
                ),
                (
                    "fragment:30:9:0".to_owned(),
                    "fragment".to_owned(),
                    Some(30),
                    Some(9)
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn direct_metadata_catalog_keeps_bounded_prefixes_out_of_payload_pack()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create_direct_metadata(&job, 4, 3)?;
        message_start(&mut sink, 10)?;
        let property = PropertyDescriptor {
            owner: PropertyOwner::Message(10),
            record_set_index: 0,
            entry_index: 0,
            entry_type: Some(0x1000),
            value_type: Some(0x001f),
            data_size: 9,
        };
        assert_eq!(sink.property_payload(property), PayloadRequest::Prefix(4));
        sink.event(CatalogEvent::PropertyStart(property))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor: property,
            bytes: b"abcd",
        })?;
        sink.event(CatalogEvent::PropertyEnd(property))?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 10,
            index: 0,
            attachment_type: Some(i32::from(b'f')),
            data_size: Some(8),
            filename: Some("payload.bin".to_owned()),
        })?;
        assert_eq!(
            sink.attachment_payload(10, 0, Some(8)),
            PayloadRequest::Prefix(3)
        );
        sink.event(CatalogEvent::AttachmentData {
            message_id: 10,
            index: 0,
            bytes: b"xyz",
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 10,
            index: 0,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.checkpoint()?;

        assert_eq!(
            fs::metadata(job.join(".pstforge/spool/payload.pack"))?.len(),
            0
        );
        let events = sink
            .spooled_candidates()?
            .pop()
            .ok_or("direct metadata candidate is absent")?
            .events;
        assert_eq!(events[0].kind, "property_direct");
        assert_eq!(events[0].metadata["direct_id"].as_u64(), Some(1));
        assert_eq!(events[0].blob.as_ref().map(|blob| blob.byte_len), Some(4));
        assert_eq!(events[2].kind, "attachment_direct");
        assert_eq!(events[2].metadata["direct_id"].as_u64(), Some(2));
        assert_eq!(events[2].metadata["declared_size"].as_u64(), Some(8));
        assert_eq!(events[2].blob.as_ref().map(|blob| blob.byte_len), Some(3));
        let mut property_prefix = Vec::new();
        sink.open_blob(events[0].blob.as_ref().ok_or("property prefix absent")?)?
            .read_to_end(&mut property_prefix)?;
        assert_eq!(property_prefix, b"abcd");
        drop(sink);

        let mut reopened = DurableCatalogSink::open(&job)?;
        reopened.enable_direct_metadata_capture(4, 3)?;
        message_start(&mut reopened, 11)?;
        let property = PropertyDescriptor {
            owner: PropertyOwner::Message(11),
            record_set_index: 0,
            entry_index: 0,
            entry_type: Some(0x1000),
            value_type: Some(0x001f),
            data_size: 5,
        };
        reopened.event(CatalogEvent::PropertyStart(property))?;
        reopened.event(CatalogEvent::PropertyData {
            descriptor: property,
            bytes: b"next",
        })?;
        reopened.event(CatalogEvent::PropertyEnd(property))?;
        reopened.event(CatalogEvent::MessageEnd {
            id: 11,
            complete: true,
        })?;
        reopened.checkpoint()?;
        let retry_event = reopened
            .spooled_candidates()?
            .into_iter()
            .find(|candidate| candidate.source_node_id == Some(11))
            .and_then(|candidate| candidate.events.into_iter().next())
            .ok_or("retry direct metadata event is absent")?;
        assert_eq!(retry_event.metadata["direct_id"].as_u64(), Some(3));
        assert_eq!(
            fs::metadata(job.join(".pstforge/spool/payload.pack"))?.len(),
            0
        );
        Ok(())
    }

    #[test]
    fn unsupported_candidate_has_distinct_durable_status() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start_with_support(
            &mut sink,
            10,
            CatalogProvenance::Normal,
            None,
            Some(1),
            false,
        )?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        let summary = sink.summary()?;
        assert_eq!(summary.committed_candidates, 1);
        assert_eq!(summary.unsupported_candidates, 1);
        let status = sink
            .connection
            .query_row("SELECT status FROM candidates", [], |row| {
                row.get::<_, String>(0)
            })?;
        assert_eq!(status, "unsupported");
        assert_eq!(
            sink.candidate_rejection_counts()?
                .get(&CandidateRejectionCategory::SourceItemUnsupported),
            Some(&1)
        );
        Ok(())
    }

    #[test]
    fn rejection_metadata_is_typed_bounded_and_strictly_validated()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.mark_candidates_unsupported(
            &["normal:10:-:0".to_owned()],
            CandidateRejectionCategory::MalformedProperty,
        )?;
        sink.checkpoint()?;
        drop(sink);
        let sink = DurableCatalogSink::open(&job)?;

        let metadata = sink.connection.query_row(
            "SELECT metadata_json FROM candidate_events \
             WHERE kind = 'output_unrepresentable'",
            [],
            |row| row.get::<_, String>(0),
        )?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&metadata)?,
            json!({"schema_version": 1, "category": "malformed_property"})
        );
        assert!(!metadata.contains("private subject"));
        assert_eq!(
            sink.candidate_rejection_counts()?
                .get(&CandidateRejectionCategory::MalformedProperty),
            Some(&1)
        );

        sink.connection.execute(
            "UPDATE candidate_events SET metadata_json = '{}' \
             WHERE kind = 'output_unrepresentable'",
            [],
        )?;
        assert!(matches!(
            sink.candidate_rejection_counts(),
            Err(JobError::Integrity(_))
        ));
        sink.connection.execute(
            "UPDATE candidate_events SET metadata_json = ?1 \
             WHERE kind = 'output_unrepresentable'",
            [metadata],
        )?;
        sink.connection.execute(
            "UPDATE candidates SET status = 'spooled' \
             WHERE item_key = 'normal:10:-:0'",
            [],
        )?;
        assert!(matches!(
            sink.candidate_rejection_counts(),
            Err(JobError::Integrity(_))
        ));
        Ok(())
    }

    #[test]
    fn named_property_catalog_is_stable_after_candidates_become_terminal()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        for (id, name) in [(10, 0x1001), (11, 0x1002)] {
            message_start(&mut sink, id)?;
            let descriptor = PropertyDescriptor {
                owner: PropertyOwner::Message(id),
                record_set_index: 0,
                entry_index: 0,
                entry_type: Some(0x8001),
                value_type: Some(0x0003),
                data_size: 4,
            };
            sink.event(CatalogEvent::NamedProperty {
                descriptor,
                identity: NamedPropertyIdentity {
                    guid: [u8::try_from(id)?; 16],
                    name: NamedPropertyName::Numeric(name),
                },
            })?;
            sink.event(CatalogEvent::PropertyStart(descriptor))?;
            sink.event(CatalogEvent::PropertyData {
                descriptor,
                bytes: &42_i32.to_le_bytes(),
            })?;
            sink.event(CatalogEvent::PropertyEnd(descriptor))?;
            sink.event(CatalogEvent::MessageEnd { id, complete: true })?;
        }
        let interrupted = AtomicBool::new(false);
        let before = sink.candidate_named_property_identities_interruptible(&interrupted)?;

        sink.mark_candidates_unsupported(
            &["normal:10:-:0".to_owned()],
            CandidateRejectionCategory::MalformedCandidate,
        )?;
        sink.connection.execute(
            "UPDATE candidates SET status = 'failed' WHERE item_key = 'normal:11:-:0'",
            [],
        )?;

        let after = sink.candidate_named_property_identities_interruptible(&interrupted)?;
        assert_eq!(after, before);
        Ok(())
    }

    #[test]
    fn top_level_header_cursor_pages_without_loading_embedded_candidates()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        let transaction = sink.connection.transaction()?;
        for id in 1_u32..=1_025 {
            transaction.execute(
                "INSERT INTO candidates(\
                    item_key, provenance, source_node_id, occurrence, completeness, status,\
                    metadata_json\
                 ) VALUES (?1, 'normal', ?2, 0, 'complete', 'spooled', ?3)",
                rusqlite::params![
                    format!("normal:{id}:-:0"),
                    id,
                    serde_json::to_string(&json!({
                        "parent_message_id": null,
                        "parent_attachment_index": null,
                        "embedded_path": []
                    }))?
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO candidates(\
                item_key, provenance, source_node_id, occurrence, completeness, status,\
                metadata_json, parent_item_key, parent_attachment_index, embedded_path_json\
             ) VALUES ('normal:2000:-:0', 'normal', 2000, 0, 'complete', 'spooled', ?1,\
                       'normal:1:-:0', 0, '[0]')",
            [serde_json::to_string(&json!({
                "parent_message_id": 1,
                "parent_attachment_index": 0,
                "embedded_path": [0]
            }))?],
        )?;
        transaction.commit()?;

        let interrupted = AtomicBool::new(false);
        let first =
            sink.spooled_top_level_candidate_headers_page_interruptible(0, 1_024, &interrupted)?;
        assert_eq!(first.tree.candidates.len(), 1_024);
        assert_eq!(first.tree.ownerships.len(), 1_024);
        let second = sink.spooled_top_level_candidate_headers_page_interruptible(
            first.next_rowid,
            1_024,
            &interrupted,
        )?;
        assert_eq!(second.tree.candidates.len(), 1);
        assert_eq!(second.tree.ownerships.len(), 1);
        let end = sink.spooled_top_level_candidate_headers_page_interruptible(
            second.next_rowid,
            1_024,
            &interrupted,
        )?;
        assert!(end.tree.candidates.is_empty());

        sink.connection.execute(
            "UPDATE candidates SET status = 'written' WHERE item_key = 'normal:1:-:0'",
            [],
        )?;
        sink.connection.execute(
            "UPDATE candidates SET status = 'unsupported' \
             WHERE item_key = 'normal:2000:-:0'",
            [],
        )?;
        let embedded =
            sink.direct_embedded_candidates_interruptible("normal:1:-:0", &interrupted)?;
        assert_eq!(
            embedded,
            vec![DirectEmbeddedCandidate {
                item_key: "normal:2000:-:0".to_owned(),
                provenance: CatalogProvenance::Normal,
                source_node_id: Some(2_000),
                recovery_index: None,
                parent_item_key: "normal:1:-:0".to_owned(),
                parent_attachment_index: 0,
            }]
        );
        Ok(())
    }

    #[test]
    fn worker_supervision_events_survive_reopen() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.record_worker_event("started", 1, "parser")?;
        sink.record_worker_event("failure", 1, "stall")?;
        sink.record_worker_supervision(1, 1, true)?;
        drop(sink);
        let reopened = DurableCatalogSink::open(&job)?;
        assert_eq!(reopened.worker_supervision()?, (1, 1));
        assert!(reopened.worker_retries_exhausted()?);
        assert_eq!(
            reopened.worker_events()?,
            vec![
                WorkerEvent {
                    kind: "started".to_owned(),
                    attempt: 1,
                    category: "parser".to_owned(),
                },
                WorkerEvent {
                    kind: "failure".to_owned(),
                    attempt: 1,
                    category: "stall".to_owned(),
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn terminal_written_parent_strands_and_accounts_embedded_descendants()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        sink.connection.execute(
            "INSERT INTO candidates(\
                item_key, provenance, source_node_id, occurrence, completeness, status,\
                metadata_json\
             ) VALUES ('normal:1:-:0', 'normal', 1, 0, 'partial', 'written', '{}')",
            [],
        )?;
        sink.connection.execute(
            "INSERT INTO candidates(\
                item_key, provenance, source_node_id, occurrence, completeness, status,\
                metadata_json, parent_item_key, parent_attachment_index, embedded_path_json\
             ) VALUES ('normal:2:-:0', 'normal', 2, 0, 'complete', 'spooled', '{}',\
                       'normal:1:-:0', 0, '[0]')",
            [],
        )?;
        sink.connection.execute(
            "INSERT INTO candidates(\
                item_key, provenance, source_node_id, occurrence, completeness, status,\
                metadata_json, parent_item_key, parent_attachment_index, embedded_path_json\
             ) VALUES ('normal:3:-:0', 'normal', 3, 0, 'complete', 'spooled', '{}',\
                       'normal:2:-:0', 0, '[0,0]')",
            [],
        )?;

        assert_eq!(sink.mark_stranded_embedded_candidates_unsupported()?, 2);
        let statuses = sink
            .connection
            .prepare("SELECT status FROM candidates ORDER BY source_node_id")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(statuses, ["written", "unsupported", "unsupported"]);
        assert_eq!(
            sink.candidate_rejection_counts()?
                .get(&CandidateRejectionCategory::StrandedEmbeddedItem),
            Some(&2)
        );
        assert_eq!(sink.mark_stranded_embedded_candidates_unsupported()?, 0);
        Ok(())
    }

    fn body_descriptor(id: u32, size: u64) -> PropertyDescriptor {
        PropertyDescriptor {
            owner: PropertyOwner::Message(id),
            record_set_index: 0,
            entry_index: 0,
            entry_type: Some(0x1000),
            value_type: Some(0x001f),
            data_size: size,
        }
    }

    #[test]
    fn committed_candidates_and_blobs_survive_reopen() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        sink.event(CatalogEvent::Folder {
            id: 1,
            parent_id: None,
            name: Some("private folder".to_owned()),
            container_class: Some("IPF.Note".to_owned()),
        })?;
        message_start(&mut sink, 10)?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 10,
            index: 0,
            attachment_type: Some(i32::from(b'd')),
            data_size: Some(6),
            filename: Some("private.bin".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentData {
            message_id: 10,
            index: 0,
            bytes: b"abc",
        })?;
        sink.event(CatalogEvent::AttachmentData {
            message_id: 10,
            index: 0,
            bytes: b"def",
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 10,
            index: 0,
        })?;
        let descriptor = body_descriptor(10, 4);
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: b"body",
        })?;
        sink.event(CatalogEvent::PropertyEnd(descriptor))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.checkpoint()?;
        drop(sink);

        let reopened = DurableCatalogSink::open(&job)?;
        let summary = reopened.summary()?;
        assert_eq!(summary.committed_candidates, 1);
        assert_eq!(summary.complete_candidates, 1);
        assert_eq!(summary.blob_count, 2);
        assert_eq!(summary.blob_bytes, 10);
        Ok(())
    }

    #[test]
    fn recovery_log_is_private_atomic_and_replaces_a_symlink_without_following_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.publish_recovery_log("first\n")?;
        let log = job.join("recovery.log");
        assert_eq!(std::fs::read_to_string(&log)?, "first\n");
        assert_eq!(log.metadata()?.permissions().mode() & 0o777, 0o600);

        let external = directory.path().join("external");
        std::fs::write(&external, "untouched\n")?;
        std::fs::remove_file(&log)?;
        symlink(&external, &log)?;
        sink.publish_recovery_log("second\n")?;
        assert_eq!(std::fs::read_to_string(&external)?, "untouched\n");
        assert_eq!(std::fs::read_to_string(&log)?, "second\n");
        assert!(!log.symlink_metadata()?.file_type().is_symlink());
        assert_eq!(log.metadata()?.permissions().mode() & 0o777, 0o600);
        Ok(())
    }

    #[test]
    fn spool_read_model_and_part_assignment_are_transactional()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        sink.event(CatalogEvent::Folder {
            id: 1,
            parent_id: None,
            name: Some("private folder".to_owned()),
            container_class: Some("IPF.Note".to_owned()),
        })?;
        message_start(&mut sink, 10)?;
        let descriptor = body_descriptor(10, 4);
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: b"body",
        })?;
        sink.event(CatalogEvent::PropertyEnd(descriptor))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;

        let folders = sink.spooled_folders()?;
        assert_eq!(
            folders
                .iter()
                .find(|folder| folder.source_id == 1)
                .and_then(|folder| folder.name.as_deref()),
            Some("private folder")
        );
        assert_eq!(
            folders
                .iter()
                .find(|folder| folder.source_id == 1)
                .and_then(|folder| folder.container_class.as_deref()),
            Some("IPF.Note")
        );
        let candidates = sink.spooled_candidates()?;
        assert_eq!(candidates.len(), 1);
        let blob = candidates[0].events[0]
            .blob
            .as_ref()
            .ok_or("missing blob")?;
        let mut payload = Vec::new();
        sink.open_blob(blob)?.read_to_end(&mut payload)?;
        assert_eq!(payload, b"body");

        let blob_path = job.join(".pstforge/spool").join(&blob.sha256);
        std::fs::write(&blob_path, b"evil")?;
        assert!(matches!(
            validate_blob_store(&sink.connection, &sink.spool, None),
            Err(JobError::Integrity(_))
        ));
        std::fs::remove_file(&blob_path)?;

        let part_bytes = vec![0_u8; 1024];
        let part_path = job.join("parts/part-0001.pst");
        std::fs::write(&part_path, &part_bytes)?;
        std::fs::set_permissions(&part_path, std::fs::Permissions::from_mode(0o600))?;
        let part = PublishedPart {
            index: 1,
            filename: "part-0001.pst".to_owned(),
            byte_len: 1024,
            sha256: Some(digest_hex(Sha256::digest(&part_bytes).as_slice())),
            oversize: false,
        };
        let sidecar = PartSidecar {
            schema_version: "1.1.0".to_owned(),
            producer_version: "0.4.0".to_owned(),
            index: part.index,
            filename: part.filename.clone(),
            byte_len: part.byte_len,
            sha256: part.sha256.clone(),
            published_device: None,
            published_inode: None,
            store_record_key: "0".repeat(32),
            folder_count: 1,
            message_count: 1,
            oversize: part.oversize,
            partial: false,
            omitted_folders: 0,
            omitted_properties: 0,
            omitted_attachments: 0,
            reconstructions: Default::default(),
        };
        let interrupted = AtomicBool::new(true);
        assert!(matches!(
            super::verify_part_artifact(&part_path, &part, &sidecar, Some(&interrupted)),
            Err(JobError::Interrupted)
        ));
        let item_key = candidates[0].item_key.clone();
        let oversize = PublishedPart {
            oversize: true,
            ..part.clone()
        };
        let oversize_sidecar = PartSidecar {
            oversize: true,
            ..sidecar.clone()
        };
        assert!(
            sink.commit_published_part(
                &oversize,
                &oversize_sidecar,
                &[item_key.clone(), "missing".to_owned()],
            )
            .is_err()
        );
        assert!(
            sink.commit_published_part(&part, &sidecar, &[item_key.clone(), "missing".to_owned()],)
                .is_err()
        );
        assert_eq!(sink.spooled_candidates()?.len(), 1);
        sink.commit_published_part(&part, &sidecar, &[item_key])?;
        let sidecar_path = job.join(".pstforge/manifests/part-0001.json");
        std::fs::write(&sidecar_path, serde_json::to_vec(&sidecar)?)?;
        std::fs::set_permissions(&sidecar_path, std::fs::Permissions::from_mode(0o600))?;
        assert!(sink.spooled_candidates()?.is_empty());
        assert_eq!(sink.summary()?.committed_candidates, 1);
        assert!(
            sink.commit_published_part(&part, &sidecar, &["normal:10:-:0".to_owned()],)
                .is_err()
        );
        sink.finalize_private_work(true)?;
        assert_eq!(sink.summary()?.blob_count, 1);
        assert_eq!(std::fs::read_dir(job.join(".pstforge/spool"))?.count(), 1);
        assert_eq!(
            sink.connection
                .query_row("SELECT COUNT(*) FROM inline_blobs", [], |row| row
                    .get::<_, u64>(0))?,
            0
        );
        sink.finalize_private_work(false)?;
        assert_eq!(sink.summary()?.blob_count, 1);
        assert_eq!(sink.summary()?.blob_bytes, 4);
        assert_eq!(std::fs::read_dir(job.join(".pstforge/spool"))?.count(), 0);
        assert_eq!(
            sink.connection
                .query_row("SELECT COUNT(*) FROM inline_blobs", [], |row| row
                    .get::<_, u64>(0))?,
            0
        );
        sink.checkpoint()?;
        drop(sink);
        assert!(matches!(
            DurableCatalogSink::open_interruptible(&job, &AtomicBool::new(true)),
            Err(JobError::Interrupted)
        ));
        let reopened = DurableCatalogSink::open(&job)?;
        drop(reopened);
        let mut wrong_sidecar = sidecar.clone();
        wrong_sidecar.omitted_properties = 1;
        std::fs::write(&sidecar_path, serde_json::to_vec(&wrong_sidecar)?)?;
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::Integrity(_))
        ));
        std::fs::write(&sidecar_path, serde_json::to_vec(&sidecar)?)?;
        std::fs::write(&part_path, vec![1_u8; 1024])?;
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::Integrity(_))
        ));
        Ok(())
    }

    #[test]
    fn small_blobs_are_transactional_and_writer_scratch_is_disposable()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        let descriptor = body_descriptor(10, 4);
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: b"body",
        })?;
        sink.event(CatalogEvent::PropertyEnd(descriptor))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;

        assert_eq!(std::fs::read_dir(job.join(".pstforge/spool"))?.count(), 1);
        assert_eq!(
            sink.connection
                .query_row("SELECT COUNT(*) FROM inline_blobs", [], |row| row
                    .get::<_, u64>(0))?,
            0
        );
        let candidate = sink.spooled_candidates()?.remove(0);
        let blob = candidate.events[0].blob.as_ref().ok_or("missing blob")?;
        let mut payload = Vec::new();
        sink.open_blob(blob)?.read_to_end(&mut payload)?;
        assert_eq!(payload, b"body");
        let materialized = sink.verified_blob_path(blob)?;
        assert_eq!(
            materialized.file_name().and_then(|name| name.to_str()),
            Some(PAYLOAD_PACK_FILENAME)
        );
        assert_eq!(std::fs::read(&materialized)?, b"body");
        assert!(
            !job.join(".pstforge/partial")
                .join(INLINE_CACHE_DIRECTORY)
                .exists()
        );
        sink.checkpoint()?;
        drop(sink);

        let reopened = DurableCatalogSink::open(&job)?;
        assert!(
            !job.join(".pstforge/partial")
                .join(INLINE_CACHE_DIRECTORY)
                .exists()
        );
        let blob = reopened.spooled_candidates()?.remove(0).events[0]
            .blob
            .clone()
            .ok_or("missing reopened blob")?;
        let mut payload = Vec::new();
        reopened.open_blob(&blob)?.read_to_end(&mut payload)?;
        assert_eq!(payload, b"body");
        Ok(())
    }

    #[test]
    fn empty_pack_range_may_share_the_next_payload_offset() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        let empty = body_descriptor(10, 0);
        sink.event(CatalogEvent::PropertyStart(empty))?;
        sink.event(CatalogEvent::PropertyEnd(empty))?;
        let body = PropertyDescriptor {
            entry_index: 1,
            ..body_descriptor(10, 4)
        };
        sink.event(CatalogEvent::PropertyStart(body))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor: body,
            bytes: b"body",
        })?;
        sink.event(CatalogEvent::PropertyEnd(body))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.checkpoint()?;
        drop(sink);

        let reopened = DurableCatalogSink::open(&job)?;
        assert_eq!(reopened.summary()?.blob_count, 2);
        assert_eq!(reopened.summary()?.blob_bytes, 4);
        Ok(())
    }

    #[test]
    fn normal_cleanup_erases_payload_pack_and_private_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let database = job.join(".pstforge/job.sqlite3");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        let mut payload = Vec::with_capacity(usize::try_from(INLINE_BLOB_MAX_BYTES)?);
        while payload.len() < usize::try_from(INLINE_BLOB_MAX_BYTES)? {
            payload.extend_from_slice(b"pstforge-private-inline-sentinel-");
        }
        payload.truncate(usize::try_from(INLINE_BLOB_MAX_BYTES)?);
        let descriptor = body_descriptor(10, u64::try_from(payload.len())?);
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: &payload,
        })?;
        sink.event(CatalogEvent::PropertyEnd(descriptor))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.checkpoint()?;
        let pack = job.join(".pstforge/spool").join(PAYLOAD_PACK_FILENAME);
        assert!(
            std::fs::read(&pack)?
                .windows(b"pstforge-private-inline-sentinel".len())
                .any(|window| window == b"pstforge-private-inline-sentinel")
        );
        sink.mark_candidates_unsupported(
            &["normal:10:-:0".to_owned()],
            CandidateRejectionCategory::MalformedCandidate,
        )?;
        sink.finalize_private_work(false)?;
        assert!(!pack.exists());
        sink.connection.execute(
            "UPDATE job_metadata SET value = 'true' \
             WHERE key = 'cleanup_compaction_pending'",
            [],
        )?;
        sink.finalize_private_work(false)?;
        drop(sink);
        let database_bytes = std::fs::read(database)?;
        assert!(
            !database_bytes
                .windows(b"pstforge-private-inline-sentinel".len())
                .any(|window| window == b"pstforge-private-inline-sentinel")
        );
        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        assert_eq!(
            connection.query_row(
                "SELECT value FROM job_metadata \
                 WHERE key = 'cleanup_compaction_pending'",
                [],
                |row| row.get::<_, String>(0),
            )?,
            "false"
        );
        Ok(())
    }

    #[test]
    fn long_sql_operation_honors_interruption() -> Result<(), Box<dyn std::error::Error>> {
        let mut connection = Connection::open_in_memory()?;
        let interrupted = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let result = thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(25));
                interrupted.store(true, Ordering::Relaxed);
            });
            run_sql_interruptible(&mut connection, Some(&interrupted), |connection| {
                connection.query_row(
                    "WITH RECURSIVE count(value) AS (\
                         SELECT 1 UNION ALL SELECT value + 1 FROM count WHERE value < 100000000\
                     ) SELECT SUM(value) FROM count",
                    [],
                    |row| row.get::<_, i64>(0),
                )?;
                Ok(())
            })
        });
        assert!(matches!(result, Err(JobError::Interrupted)));
        assert!(started.elapsed() < Duration::from_secs(2));
        Ok(())
    }

    #[test]
    fn publication_intent_recovers_every_cross_resource_crash_state()
    -> Result<(), Box<dyn std::error::Error>> {
        for publication_step in 0..=2 {
            let directory = tempdir()?;
            let job = directory.path().join("job");
            let mut sink = DurableCatalogSink::create(&job)?;
            message_start(&mut sink, 10)?;
            sink.event(CatalogEvent::MessageEnd {
                id: 10,
                complete: true,
            })?;
            sink.checkpoint()?;
            let item_key = sink.spooled_candidates()?[0].item_key.clone();
            let bytes = b"validated part checkpoint";
            let part = PublishedPart {
                index: 1,
                filename: "part-0001.pst".to_owned(),
                byte_len: bytes.len() as u64,
                sha256: Some(digest_hex(Sha256::digest(bytes).as_slice())),
                oversize: false,
            };
            let sidecar = PartSidecar {
                schema_version: "1.1.0".to_owned(),
                producer_version: "0.4.0".to_owned(),
                index: 1,
                filename: part.filename.clone(),
                byte_len: part.byte_len,
                sha256: part.sha256.clone(),
                published_device: None,
                published_inode: None,
                store_record_key: "0".repeat(32),
                folder_count: 1,
                message_count: 1,
                oversize: false,
                partial: false,
                omitted_folders: 0,
                omitted_properties: 0,
                omitted_attachments: 0,
                reconstructions: Default::default(),
            };
            write_sidecar_partial(
                &job.join(".pstforge/partial/part-0001.json.partial"),
                &sidecar,
            )?;
            sink.record_publication_intent(&part, &sidecar, std::slice::from_ref(&item_key))?;
            if publication_step >= 1 {
                let path = job.join("parts/part-0001.pst");
                std::fs::write(&path, bytes)?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
            if publication_step >= 2 {
                std::fs::rename(
                    job.join(".pstforge/partial/part-0001.json.partial"),
                    job.join(".pstforge/manifests/part-0001.json"),
                )?;
            }
            drop(sink);

            let reopened = DurableCatalogSink::open(&job)?;
            let intent_count = reopened.connection.query_row(
                "SELECT COUNT(*) FROM publication_intents",
                [],
                |row| row.get::<_, u64>(0),
            )?;
            assert_eq!(intent_count, 0);
            if publication_step == 0 {
                assert_eq!(reopened.spooled_candidates()?.len(), 1);
                assert!(!job.join("parts/part-0001.pst").exists());
                assert!(
                    !job.join(".pstforge/partial/part-0001.json.partial")
                        .exists()
                );
            } else {
                assert!(reopened.spooled_candidates()?.is_empty());
                assert!(job.join("parts/part-0001.pst").is_file());
                assert!(job.join(".pstforge/manifests/part-0001.json").is_file());
            }
        }
        Ok(())
    }

    #[test]
    fn constructed_publication_rejects_a_same_length_staged_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.checkpoint()?;
        let item_key = sink.spooled_candidates()?[0].item_key.clone();
        let staged_filename = "part-0001.pst.partial";
        let staged = job.join(".pstforge/partial").join(staged_filename);
        fs::write(&staged, b"direct part checkpoint")?;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o600))?;
        let original = staged.metadata()?;
        let part = PublishedPart {
            index: 1,
            filename: "part-0001.pst".to_owned(),
            byte_len: original.len(),
            sha256: None,
            oversize: false,
        };
        let sidecar = PartSidecar {
            schema_version: "1.2.0".to_owned(),
            producer_version: "0.5.0".to_owned(),
            index: part.index,
            filename: part.filename.clone(),
            byte_len: part.byte_len,
            sha256: None,
            published_device: Some(original.dev()),
            published_inode: Some(original.ino()),
            store_record_key: "0".repeat(32),
            folder_count: 1,
            message_count: 1,
            oversize: false,
            partial: false,
            omitted_folders: 0,
            omitted_properties: 0,
            omitted_attachments: 0,
            reconstructions: Default::default(),
        };
        let replacement = job.join(".pstforge/partial/replacement.pst");
        fs::write(&replacement, b"replaced part content!")?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        fs::rename(&replacement, &staged)?;

        assert!(matches!(
            sink.publish_constructed_part_interruptible(
                staged_filename,
                &part,
                &sidecar,
                &[item_key],
                &AtomicBool::new(false),
            ),
            Err(JobError::Integrity(_))
        ));
        assert!(!job.join("parts/part-0001.pst").exists());
        Ok(())
    }

    #[test]
    fn incomplete_candidates_roll_back_on_drop() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        let descriptor = body_descriptor(10, 4);
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: b"body",
        })?;
        sink.event(CatalogEvent::PropertyEnd(descriptor))?;
        drop(sink);
        let reopened = DurableCatalogSink::open(&job)?;
        assert_eq!(reopened.summary()?.committed_candidates, 0);
        assert_eq!(reopened.summary()?.blob_count, 0);
        assert_eq!(std::fs::read_dir(job.join(".pstforge/spool"))?.count(), 1);
        assert_eq!(
            job.join(".pstforge/spool")
                .join(PAYLOAD_PACK_FILENAME)
                .metadata()?
                .len(),
            0
        );
        Ok(())
    }

    #[test]
    fn folder_properties_stream_without_candidate_storage() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        sink.event(CatalogEvent::Folder {
            id: 1,
            parent_id: None,
            name: Some("folder".to_owned()),
            container_class: Some("IPF.Note".to_owned()),
        })?;
        let descriptor = PropertyDescriptor {
            owner: PropertyOwner::Folder(1),
            record_set_index: 0,
            entry_index: 0,
            entry_type: Some(0x3001),
            value_type: Some(0x001f),
            data_size: 4,
        };
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: b"name",
        })?;
        sink.event(CatalogEvent::PropertyEnd(descriptor))?;
        assert_eq!(sink.summary()?.blob_count, 0);
        Ok(())
    }

    #[test]
    fn corrupt_ledger_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        drop(sink);
        std::fs::write(job.join(".pstforge/job.sqlite3"), b"not sqlite")?;
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::Sql(_) | JobError::Integrity(_))
        ));
        Ok(())
    }

    #[test]
    fn damaged_substreams_commit_a_partial_candidate() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 10,
            index: 0,
            attachment_type: Some(i32::from(b'd')),
            data_size: Some(6),
            filename: None,
        })?;
        sink.event(CatalogEvent::AttachmentData {
            message_id: 10,
            index: 0,
            bytes: b"abc",
        })?;
        let descriptor = body_descriptor(10, 4);
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: b"bo",
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: false,
        })?;
        let summary = sink.summary()?;
        assert_eq!(summary.committed_candidates, 1);
        assert_eq!(summary.partial_candidates, 1);
        assert_eq!(summary.blob_count, 1);
        assert_eq!(summary.blob_bytes, 3);
        Ok(())
    }

    #[test]
    fn property_abort_discards_partial_bytes_and_allows_progress()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        let abandoned = body_descriptor(10, 4);
        sink.event(CatalogEvent::PropertyStart(abandoned))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor: abandoned,
            bytes: b"ba",
        })?;
        sink.event(CatalogEvent::PropertyAbort {
            descriptor: abandoned,
            reason: "injected read failure".to_owned(),
        })?;
        let retained = PropertyDescriptor {
            entry_index: 1,
            ..body_descriptor(10, 4)
        };
        sink.event(CatalogEvent::PropertyStart(retained))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor: retained,
            bytes: b"good",
        })?;
        sink.event(CatalogEvent::PropertyEnd(retained))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: false,
        })?;
        let summary = sink.summary()?;
        assert_eq!(summary.partial_candidates, 1);
        assert_eq!(summary.blob_count, 1);
        assert_eq!(summary.blob_bytes, 4);
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        let metadata = connection.query_row(
            "SELECT metadata_json FROM candidate_events WHERE kind = 'property_incomplete'",
            [],
            |row| row.get::<_, String>(0),
        )?;
        assert!(metadata.contains("injected read failure"));
        Ok(())
    }

    #[test]
    fn completed_attachment_stays_complete_when_later_message_data_is_partial()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 10,
            index: 0,
            attachment_type: Some(i32::from(b'd')),
            data_size: Some(3),
            filename: None,
        })?;
        sink.event(CatalogEvent::AttachmentData {
            message_id: 10,
            index: 0,
            bytes: b"abc",
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 10,
            index: 0,
        })?;
        let body = body_descriptor(10, 4);
        sink.event(CatalogEvent::PropertyStart(body))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor: body,
            bytes: b"bo",
        })?;
        sink.event(CatalogEvent::PropertyAbort {
            descriptor: body,
            reason: "injected read failure".to_owned(),
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: false,
        })?;
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        let kind = connection.query_row(
            "SELECT kind FROM candidate_events WHERE blob_sha256 IS NOT NULL",
            [],
            |row| row.get::<_, String>(0),
        )?;
        assert_eq!(kind, "attachment_data");
        Ok(())
    }

    #[test]
    fn aborted_attachment_does_not_block_later_attachment() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 10,
            index: 0,
            attachment_type: Some(i32::from(b'd')),
            data_size: Some(4),
            filename: Some("damaged.bin".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentData {
            message_id: 10,
            index: 0,
            bytes: b"ab",
        })?;
        sink.event(CatalogEvent::AttachmentAbort {
            message_id: 10,
            index: 0,
        })?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 10,
            index: 1,
            attachment_type: Some(i32::from(b'd')),
            data_size: Some(3),
            filename: Some("valid.bin".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentData {
            message_id: 10,
            index: 1,
            bytes: b"xyz",
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 10,
            index: 1,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: false,
        })?;

        let candidate = sink.spooled_candidates()?.remove(0);
        assert_eq!(
            candidate
                .events
                .iter()
                .filter(|event| event.kind == "attachment")
                .count(),
            2
        );
        assert!(candidate.events.iter().any(|event| {
            event.kind == "attachment_partial" && event.metadata["index"].as_u64() == Some(0)
        }));
        assert!(candidate.events.iter().any(|event| {
            event.kind == "attachment_data" && event.metadata["index"].as_u64() == Some(1)
        }));
        Ok(())
    }

    #[test]
    fn complete_attachment_requires_its_explicit_end_event()
    -> Result<(), Box<dyn std::error::Error>> {
        for end_with_message in [false, true] {
            let directory = tempdir()?;
            let job = directory.path().join("job");
            let mut sink = DurableCatalogSink::create(&job)?;
            message_start(&mut sink, 10)?;
            sink.event(CatalogEvent::AttachmentStart {
                message_id: 10,
                index: 0,
                attachment_type: Some(i32::from(b'd')),
                data_size: None,
                filename: None,
            })?;
            let rejected = if end_with_message {
                sink.event(CatalogEvent::MessageEnd {
                    id: 10,
                    complete: true,
                })
            } else {
                sink.event(CatalogEvent::AttachmentStart {
                    message_id: 10,
                    index: 1,
                    attachment_type: Some(i32::from(b'd')),
                    data_size: Some(0),
                    filename: None,
                })
            };
            assert!(rejected.is_err());
        }
        Ok(())
    }

    #[test]
    fn incomplete_empty_attachment_is_durably_distinct_from_explicit_end()
    -> Result<(), Box<dyn std::error::Error>> {
        for expected in [None, Some(0)] {
            for explicitly_ended in [false, true] {
                let directory = tempdir()?;
                let job = directory.path().join("job");
                let mut sink = DurableCatalogSink::create(&job)?;
                message_start(&mut sink, 10)?;
                sink.event(CatalogEvent::AttachmentStart {
                    message_id: 10,
                    index: 0,
                    attachment_type: Some(i32::from(b'd')),
                    data_size: expected,
                    filename: None,
                })?;
                if explicitly_ended {
                    let ended = sink.event(CatalogEvent::AttachmentEnd {
                        message_id: 10,
                        index: 0,
                    });
                    if expected.is_none() {
                        assert!(ended.is_err());
                        continue;
                    }
                    ended?;
                }
                sink.event(CatalogEvent::MessageEnd {
                    id: 10,
                    complete: false,
                })?;
                sink.checkpoint()?;
                drop(sink);
                let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
                let missing = connection.query_row(
                    "SELECT COUNT(*) FROM candidate_events WHERE kind = 'attachment_missing'",
                    [],
                    |row| row.get::<_, u64>(0),
                )?;
                assert_eq!(missing, u64::from(!explicitly_ended));
                let data = connection.query_row(
                    "SELECT COUNT(*) FROM candidate_events WHERE kind = 'attachment_data'",
                    [],
                    |row| row.get::<_, u64>(0),
                )?;
                assert_eq!(data, u64::from(explicitly_ended));
            }
        }
        Ok(())
    }

    #[test]
    fn symlinked_job_and_replaced_spool_are_refused() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let target = directory.path().join("target");
        std::fs::create_dir(&target)?;
        let linked_job = directory.path().join("linked-job");
        symlink(&target, &linked_job)?;
        assert!(matches!(
            DurableCatalogSink::create(&linked_job),
            Err(JobError::UnsafePath(_))
        ));

        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        drop(sink);
        let original_spool = job.join(".pstforge/spool");
        std::fs::rename(&original_spool, job.join(".pstforge/original-spool"))?;
        let external = directory.path().join("external");
        std::fs::create_dir(&external)?;
        std::fs::write(external.join(".tmp-private"), b"keep")?;
        symlink(&external, &original_spool)?;
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::UnsafePath(_))
        ));
        assert_eq!(std::fs::read(external.join(".tmp-private"))?, b"keep");
        Ok(())
    }

    #[test]
    fn missing_or_wrong_blob_is_refused_on_reopen() -> Result<(), Box<dyn std::error::Error>> {
        for remove in [false, true] {
            let directory = tempdir()?;
            let job = directory.path().join("job");
            let mut sink = DurableCatalogSink::create(&job)?;
            message_start(&mut sink, 10)?;
            let payload = vec![0x5a; usize::try_from(INLINE_BLOB_MAX_BYTES)? + 1];
            let descriptor = body_descriptor(10, u64::try_from(payload.len())?);
            sink.event(CatalogEvent::PropertyStart(descriptor))?;
            sink.event(CatalogEvent::PropertyData {
                descriptor,
                bytes: &payload,
            })?;
            sink.event(CatalogEvent::PropertyEnd(descriptor))?;
            sink.event(CatalogEvent::MessageEnd {
                id: 10,
                complete: true,
            })?;
            sink.checkpoint()?;
            drop(sink);
            let spool = job.join(".pstforge/spool");
            let blob = std::fs::read_dir(&spool)?
                .next()
                .ok_or("missing committed blob")??
                .path();
            if remove {
                std::fs::remove_file(&blob)?;
            } else {
                std::fs::write(&blob, b"evil")?;
            }
            assert!(matches!(
                DurableCatalogSink::open(&job),
                Err(JobError::Io { .. } | JobError::Integrity(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn existing_schema_gains_pack_column_and_indexes_on_resume()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        let payload = vec![0x5a; usize::try_from(INLINE_BLOB_MAX_BYTES)? + 1];
        let descriptor = body_descriptor(10, u64::try_from(payload.len())?);
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: &payload,
        })?;
        sink.event(CatalogEvent::PropertyEnd(descriptor))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        connection.execute_batch(
            "DROP TABLE inline_blobs;\
             DROP INDEX candidate_events_blob_sha256;\
             DROP INDEX candidates_occurrence;\
             DROP INDEX candidates_parent_item_key;",
        )?;
        drop(connection);

        let mut resumed = DurableCatalogSink::open(&job)?;
        assert_eq!(
            resumed
                .connection
                .query_row("SELECT COUNT(*) FROM inline_blobs", [], |row| row
                    .get::<_, u64>(0))?,
            0
        );
        let mut plan = resumed.connection.prepare(
            "EXPLAIN QUERY PLAN SELECT 1 FROM blobs b WHERE NOT EXISTS(\
                SELECT 1 FROM candidate_events e WHERE e.blob_sha256 = b.sha256\
             )",
        )?;
        let details = plan
            .query_map([], |row| row.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("candidate_events_blob_sha256")),
            "orphan-blob validation must use its blob-reference index: {details:?}"
        );
        drop(plan);
        let mut plan = resumed.connection.prepare(
            "EXPLAIN QUERY PLAN SELECT COUNT(*) FROM candidates \
             WHERE provenance = ?1 AND source_node_id IS ?2 AND recovery_index IS ?3",
        )?;
        let details = plan
            .query_map(rusqlite::params!["normal", 10_i64, None::<i64>], |row| {
                row.get::<_, String>(3)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("candidates_occurrence")),
            "candidate occurrence assignment must use its identity index: {details:?}"
        );
        drop(plan);
        let mut plan = resumed.connection.prepare(
            "EXPLAIN QUERY PLAN SELECT item_key FROM candidates \
             WHERE parent_item_key = ?1",
        )?;
        let details = plan
            .query_map(["normal:10:-:0"], |row| row.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("candidates_parent_item_key")),
            "bounded candidate-tree traversal must use its parent index: {details:?}"
        );
        drop(plan);
        let mut plan = resumed.connection.prepare(
            "EXPLAIN QUERY PLAN WITH RECURSIVE tree(item_key) AS (\
                SELECT item_key FROM candidates WHERE item_key = ?1 \
                UNION SELECT child.item_key FROM candidates child \
                JOIN tree parent ON child.parent_item_key = parent.item_key\
             ) SELECT e.item_key FROM tree t \
             CROSS JOIN candidate_events e ON e.item_key = t.item_key",
        )?;
        let details = plan
            .query_map(["normal:10:-:0"], |row| row.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("candidates_parent_item_key"))
                && details
                    .iter()
                    .any(|detail| { detail.contains("sqlite_autoindex_candidate_events_1") })
                && !details.iter().any(|detail| detail == "SCAN e"),
            "bounded candidate trees must use direct child and event lookups: {details:?}"
        );
        drop(plan);
        message_start(&mut resumed, 11)?;
        let descriptor = body_descriptor(11, 4);
        resumed.event(CatalogEvent::PropertyStart(descriptor))?;
        resumed.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: b"body",
        })?;
        resumed.event(CatalogEvent::PropertyEnd(descriptor))?;
        resumed.event(CatalogEvent::MessageEnd {
            id: 11,
            complete: true,
        })?;
        assert_eq!(
            resumed
                .connection
                .query_row("SELECT COUNT(*) FROM inline_blobs", [], |row| row
                    .get::<_, u64>(0))?,
            0
        );
        Ok(())
    }

    #[test]
    fn corrupt_legacy_inline_blob_is_refused_on_reopen() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        let descriptor = body_descriptor(10, 4);
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor,
            bytes: b"body",
        })?;
        sink.event(CatalogEvent::PropertyEnd(descriptor))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        connection.execute("UPDATE blobs SET pack_offset = NULL", [])?;
        connection.execute(
            "INSERT INTO inline_blobs(sha256, data) \
             SELECT sha256, ?1 FROM blobs",
            [b"evil".as_slice()],
        )?;
        drop(connection);
        std::fs::write(job.join(".pstforge/spool").join(PAYLOAD_PACK_FILENAME), [])?;
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::Integrity(_))
        ));
        Ok(())
    }

    #[test]
    fn corrupt_existing_blob_is_not_reused() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        let first = body_descriptor(10, 4);
        sink.event(CatalogEvent::PropertyStart(first))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor: first,
            bytes: b"body",
        })?;
        sink.event(CatalogEvent::PropertyEnd(first))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        {
            let mut pack = OpenOptions::new()
                .write(true)
                .open(job.join(".pstforge/spool").join(PAYLOAD_PACK_FILENAME))?;
            pack.write_all(b"evil")?;
        }

        message_start(&mut sink, 11)?;
        let second = body_descriptor(11, 4);
        sink.event(CatalogEvent::PropertyStart(second))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor: second,
            bytes: b"body",
        })?;
        assert!(sink.event(CatalogEvent::PropertyEnd(second)).is_err());
        Ok(())
    }

    #[test]
    fn consumed_blob_is_not_rehashed_until_a_candidate_attempts_reuse()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        let payload = vec![0x5a; usize::try_from(INLINE_BLOB_MAX_BYTES)? + 1];
        let first = body_descriptor(10, u64::try_from(payload.len())?);
        sink.event(CatalogEvent::PropertyStart(first))?;
        sink.event(CatalogEvent::PropertyData {
            descriptor: first,
            bytes: &payload,
        })?;
        sink.event(CatalogEvent::PropertyEnd(first))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.mark_candidates_unsupported(
            &["normal:10:-:0".to_owned()],
            CandidateRejectionCategory::MalformedCandidate,
        )?;
        sink.checkpoint()?;
        drop(sink);

        let pack = job.join(".pstforge/spool").join(PAYLOAD_PACK_FILENAME);
        OpenOptions::new()
            .write(true)
            .open(&pack)?
            .write_all(b"evil")?;
        let mut reopened = DurableCatalogSink::open(&job)?;

        message_start(&mut reopened, 11)?;
        let second = body_descriptor(11, u64::try_from(payload.len())?);
        reopened.event(CatalogEvent::PropertyStart(second))?;
        reopened.event(CatalogEvent::PropertyData {
            descriptor: second,
            bytes: &payload,
        })?;
        assert!(reopened.event(CatalogEvent::PropertyEnd(second)).is_err());
        Ok(())
    }

    #[test]
    fn logical_foreign_key_corruption_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        connection.execute_batch("PRAGMA foreign_keys = OFF")?;
        connection.execute(
            "INSERT INTO candidate_events(\
                item_key, sequence, kind, metadata_json, blob_sha256, byte_len\
             ) VALUES ('missing', 1, 'test', '{}', NULL, NULL)",
            [],
        )?;
        drop(connection);
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::Integrity(_))
        ));
        Ok(())
    }

    #[test]
    fn inconsistent_written_part_state_is_refused_on_reopen()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 10)?;
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        connection.execute(
            "UPDATE candidates SET status = 'written' WHERE item_key = 'normal:10:-:0'",
            [],
        )?;
        drop(connection);
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::Integrity(_))
        ));
        Ok(())
    }

    #[test]
    fn event_blob_length_nullability_corruption_is_refused()
    -> Result<(), Box<dyn std::error::Error>> {
        for (hash, length) in [(None, Some(0_i64)), (Some("a".repeat(64)), None)] {
            let directory = tempdir()?;
            let job = directory.path().join("job");
            let sink = DurableCatalogSink::create(&job)?;
            sink.checkpoint()?;
            drop(sink);
            let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
            connection.execute_batch(
                "PRAGMA foreign_keys = OFF; PRAGMA ignore_check_constraints = ON;",
            )?;
            connection.execute(
                "INSERT INTO candidates(\
                    item_key, provenance, source_node_id, recovery_index, occurrence,\
                    completeness, status, metadata_json\
                 ) VALUES ('normal:1:-:0', 'normal', 1, NULL, 0, 'complete', 'spooled', '{}')",
                [],
            )?;
            connection.execute(
                "INSERT INTO candidate_events(\
                    item_key, sequence, kind, metadata_json, blob_sha256, byte_len\
                 ) VALUES ('normal:1:-:0', 1, 'test', '{}', ?1, ?2)",
                rusqlite::params![hash, length],
            )?;
            drop(connection);
            assert!(matches!(
                DurableCatalogSink::open(&job),
                Err(JobError::Integrity(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn partial_write_hashes_only_bytes_confirmed_written() {
        let mut writer = PrefixThenError { remaining: 3 };
        let mut hasher = Sha256::new();
        let mut total = 0;
        let error = write_hashed(&mut writer, &mut hasher, &mut total, b"abcdef")
            .expect_err("the injected storage failure must propagate");
        assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
        assert_eq!(total, 3);
        assert_eq!(
            digest_hex(hasher.finalize().as_slice()),
            digest_hex(Sha256::digest(b"abc").as_slice())
        );
    }

    #[test]
    fn hard_linked_sqlite_state_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        for name in ["job.sqlite3", "job.sqlite3-wal", "job.sqlite3-shm"] {
            let directory = tempdir()?;
            let job = directory.path().join("job");
            let sink = DurableCatalogSink::create(&job)?;
            sink.checkpoint()?;
            drop(sink);
            let state = job.join(".pstforge").join(name);
            if name.ends_with("-wal") || name.ends_with("-shm") {
                std::fs::write(&state, b"untrusted sidecar")?;
                std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o600))?;
            }
            std::fs::hard_link(&state, directory.path().join("outside-link"))?;
            assert!(matches!(
                DurableCatalogSink::open(&job),
                Err(JobError::UnsafePath(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn non_private_job_state_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        for target in [
            "",
            ".pstforge",
            ".pstforge/spool",
            ".pstforge/partial",
            ".pstforge/job.sqlite3",
            ".pstforge/job.sqlite3-wal",
            ".pstforge/job.sqlite3-shm",
        ] {
            let directory = tempdir()?;
            let job = directory.path().join("job");
            let sink = DurableCatalogSink::create(&job)?;
            sink.checkpoint()?;
            drop(sink);
            if target.ends_with("-wal") || target.ends_with("-shm") {
                std::fs::write(job.join(target), b"untrusted sidecar")?;
                std::fs::set_permissions(job.join(target), std::fs::Permissions::from_mode(0o600))?;
            }
            std::fs::set_permissions(
                job.join(target),
                std::fs::Permissions::from_mode(if target.ends_with("sqlite3") {
                    0o644
                } else {
                    0o755
                }),
            )?;
            assert!(matches!(
                DurableCatalogSink::open(&job),
                Err(JobError::UnsafePath(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn foreign_owned_private_state_is_rejected_by_the_attribute_gate() {
        let effective = rustix::process::geteuid().as_raw();
        let foreign = effective ^ 1;
        assert!(!private_state_attributes_valid(true, foreign, 0o700, None));
        assert!(!private_state_attributes_valid(
            true,
            foreign,
            0o600,
            Some(1)
        ));
    }

    #[test]
    fn blob_key_traversal_is_rejected_before_path_use() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        connection
            .execute_batch("PRAGMA foreign_keys = OFF; PRAGMA ignore_check_constraints = ON;")?;
        connection.execute(
            "INSERT INTO candidates(\
                item_key, provenance, source_node_id, recovery_index, occurrence,\
                completeness, status, metadata_json\
             ) VALUES ('normal:1:-:0', 'normal', 1, NULL, 0, 'complete', 'spooled', '{}')",
            [],
        )?;
        let malicious = format!("../{}", "a".repeat(61));
        assert_eq!(malicious.len(), 64);
        connection.execute(
            "INSERT INTO blobs(sha256, byte_len) VALUES (?1, 0)",
            [&malicious],
        )?;
        connection.execute(
            "INSERT INTO candidate_events(\
                item_key, sequence, kind, metadata_json, blob_sha256, byte_len\
             ) VALUES ('normal:1:-:0', 1, 'property', '{}', ?1, 0)",
            [&malicious],
        )?;
        drop(connection);
        assert!(matches!(
            DurableCatalogSink::open(&job),
            Err(JobError::Integrity(_))
        ));
        Ok(())
    }

    #[test]
    fn property_ownership_is_checked_before_spooling() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        let outside = body_descriptor(10, 4);
        assert!(sink.event(CatalogEvent::PropertyStart(outside)).is_err());
        assert_eq!(std::fs::read_dir(job.join(".pstforge/spool"))?.count(), 1);

        message_start(&mut sink, 10)?;
        let wrong_message = body_descriptor(11, 4);
        assert!(
            sink.event(CatalogEvent::PropertyStart(wrong_message))
                .is_err()
        );
        let wrong_attachment = PropertyDescriptor {
            owner: PropertyOwner::Attachment {
                message_id: 10,
                index: 0,
            },
            ..body_descriptor(10, 4)
        };
        assert!(
            sink.event(CatalogEvent::PropertyStart(wrong_attachment))
                .is_err()
        );
        sink.event(CatalogEvent::Recipient {
            message_id: 10,
            index: 0,
            recipient_type: None,
            display_name: None,
            email_address: None,
            address_type: None,
        })?;
        let wrong_recipient = PropertyDescriptor {
            owner: PropertyOwner::Recipient {
                message_id: 10,
                index: 1,
            },
            ..body_descriptor(10, 4)
        };
        assert!(
            sink.event(CatalogEvent::PropertyStart(wrong_recipient))
                .is_err()
        );
        sink.event(CatalogEvent::MessageEnd {
            id: 10,
            complete: true,
        })?;
        assert_eq!(sink.summary()?.committed_candidates, 1);
        assert_eq!(sink.summary()?.blob_count, 0);
        Ok(())
    }

    #[test]
    fn non_private_or_hard_linked_blob_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        for hard_link in [false, true] {
            let directory = tempdir()?;
            let job = directory.path().join("job");
            let mut sink = DurableCatalogSink::create(&job)?;
            message_start(&mut sink, 10)?;
            let payload = vec![0x5a; usize::try_from(INLINE_BLOB_MAX_BYTES)? + 1];
            let descriptor = body_descriptor(10, u64::try_from(payload.len())?);
            sink.event(CatalogEvent::PropertyStart(descriptor))?;
            sink.event(CatalogEvent::PropertyData {
                descriptor,
                bytes: &payload,
            })?;
            sink.event(CatalogEvent::PropertyEnd(descriptor))?;
            sink.event(CatalogEvent::MessageEnd {
                id: 10,
                complete: true,
            })?;
            sink.checkpoint()?;
            drop(sink);
            let blob = job.join(".pstforge/spool").join(PAYLOAD_PACK_FILENAME);
            if hard_link {
                std::fs::hard_link(&blob, directory.path().join("outside-blob"))?;
            } else {
                std::fs::set_permissions(&blob, std::fs::Permissions::from_mode(0o644))?;
            }
            match DurableCatalogSink::open(&job) {
                Err(JobError::Integrity(_) | JobError::UnsafePath(_)) => {}
                Err(error) => panic!("modified payload pack returned wrong error: {error}"),
                Ok(_) => panic!("modified payload pack was not refused"),
            }
        }
        Ok(())
    }

    #[test]
    fn committed_candidate_survives_sigkill() -> Result<(), Box<dyn std::error::Error>> {
        const CHILD_ENV: &str = "PSTFORGE_JOB_SIGKILL_CHILD";
        if let Some(job) = std::env::var_os(CHILD_ENV) {
            let mut sink = DurableCatalogSink::create(std::path::Path::new(&job))?;
            message_start(&mut sink, 10)?;
            sink.event(CatalogEvent::MessageEnd {
                id: 10,
                complete: true,
            })?;
            sink.checkpoint()?;
            println!("PSTFORGE_COMMITTED");
            std::io::stdout().flush()?;
            std::thread::sleep(Duration::from_secs(60));
            return Ok(());
        }

        let directory = tempdir()?;
        let job = directory.path().join("job");
        let executable = std::env::current_exe()?;
        let mut child = Command::new(executable)
            .arg("--exact")
            .arg("tests::committed_candidate_survives_sigkill")
            .arg("--nocapture")
            .env(CHILD_ENV, &job)
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().ok_or("child stdout missing")?;
        let mut ready = false;
        for line in BufReader::new(stdout).lines() {
            if line?.contains("PSTFORGE_COMMITTED") {
                ready = true;
                break;
            }
        }
        if !ready {
            return Err("child exited before committing".into());
        }
        child.kill()?;
        let _ = child.wait()?;
        let reopened = DurableCatalogSink::open(&job)?;
        assert_eq!(reopened.summary()?.committed_candidates, 1);
        Ok(())
    }

    #[test]
    fn automatic_candidate_checkpoint_bounds_sigkill_replay()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        for id in 1..=CANDIDATE_CHECKPOINT_BATCH + 1 {
            message_start(&mut sink, id)?;
            sink.event(CatalogEvent::MessageEnd { id, complete: true })?;
        }
        drop(sink);

        let reopened = DurableCatalogSink::open(&job)?;
        assert_eq!(
            reopened.summary()?.committed_candidates,
            u64::from(CANDIDATE_CHECKPOINT_BATCH)
        );
        Ok(())
    }

    #[test]
    fn direct_candidates_commit_only_at_explicit_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let uncommitted_job = directory.path().join("uncommitted");
        let mut sink = DurableCatalogSink::create_direct_metadata(&uncommitted_job, 4, 3)?;
        for id in 1..=CANDIDATE_CHECKPOINT_BATCH + 1 {
            message_start(&mut sink, id)?;
            sink.event(CatalogEvent::MessageEnd { id, complete: true })?;
        }
        drop(sink);
        assert_eq!(
            DurableCatalogSink::open(&uncommitted_job)?
                .summary()?
                .committed_candidates,
            0
        );

        let committed_job = directory.path().join("committed");
        let mut sink = DurableCatalogSink::create_direct_metadata(&committed_job, 4, 3)?;
        for id in 1..=CANDIDATE_CHECKPOINT_BATCH + 1 {
            message_start(&mut sink, id)?;
            sink.event(CatalogEvent::MessageEnd { id, complete: true })?;
        }
        sink.checkpoint()?;
        drop(sink);
        assert_eq!(
            DurableCatalogSink::open(&committed_job)?
                .summary()?
                .committed_candidates,
            u64::from(CANDIDATE_CHECKPOINT_BATCH + 1)
        );
        Ok(())
    }

    #[test]
    fn direct_capture_uses_large_bounded_cache_with_spill_enabled()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let restartable_job = directory.path().join("restartable");
        let restartable = DurableCatalogSink::create(&restartable_job)?;
        let restartable_spill =
            restartable
                .connection
                .query_row("PRAGMA cache_spill", [], |row| row.get::<_, i64>(0))?;
        assert_ne!(restartable_spill, 0);

        let direct_job = directory.path().join("direct");
        let direct = DurableCatalogSink::create_direct_metadata(&direct_job, 4, 3)?;
        let direct_spill = direct
            .connection
            .query_row("PRAGMA cache_spill", [], |row| row.get::<_, i64>(0))?;
        let direct_cache = direct
            .connection
            .query_row("PRAGMA cache_size", [], |row| row.get::<_, i64>(0))?;
        assert_ne!(direct_spill, 0);
        assert_eq!(direct_cache, -i64::try_from(DIRECT_SQLITE_CACHE_KIB)?);

        let converted_job = directory.path().join("converted");
        let mut converted = DurableCatalogSink::create(&converted_job)?;
        converted.enable_direct_metadata_capture(4, 3)?;
        let converted_spill = converted
            .connection
            .query_row("PRAGMA cache_spill", [], |row| row.get::<_, i64>(0))?;
        let converted_cache = converted
            .connection
            .query_row("PRAGMA cache_size", [], |row| row.get::<_, i64>(0))?;
        assert_ne!(converted_spill, 0);
        assert_eq!(converted_cache, -i64::try_from(DIRECT_SQLITE_CACHE_KIB)?);
        Ok(())
    }

    #[test]
    fn payload_pack_cursor_tracks_append_dedup_and_reopen() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        for id in [10, 20] {
            message_start(&mut sink, id)?;
            let descriptor = body_descriptor(id, 3);
            sink.event(CatalogEvent::PropertyStart(descriptor))?;
            sink.event(CatalogEvent::PropertyData {
                descriptor,
                bytes: b"abc",
            })?;
            sink.event(CatalogEvent::PropertyEnd(descriptor))?;
            sink.event(CatalogEvent::MessageEnd { id, complete: true })?;
        }
        sink.checkpoint()?;
        assert_eq!(
            sink.payload_pack_metrics()?,
            PayloadPackMetrics {
                current_bytes: 3,
                peak_bytes: 6,
                bytes_written: 6,
            }
        );
        drop(sink);

        let reopened = DurableCatalogSink::open(&job)?;
        assert_eq!(
            reopened.payload_pack_metrics()?,
            PayloadPackMetrics {
                current_bytes: 3,
                peak_bytes: 3,
                bytes_written: 0,
            }
        );
        Ok(())
    }
}
