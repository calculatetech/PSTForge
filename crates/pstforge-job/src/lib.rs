#![deny(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek as _, Write as _};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use libpff_sys::{
    CatalogEvent, CatalogProvenance, CatalogSink, PropertyDescriptor, PropertyOwner, RecoveryUnit,
};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

const JOB_SCHEMA_VERSION: i64 = 4;

#[derive(Debug, Error)]
pub enum JobError {
    #[error("job directory already exists and is not empty: {0}")]
    ExistingJob(PathBuf),
    #[error("unsafe or replaced job path: {0}")]
    UnsafePath(PathBuf),
    #[error("job ledger integrity check failed: {0}")]
    Integrity(String),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayCandidate {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpooledBlob {
    pub sha256: String,
    pub byte_len: u64,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublishedPart {
    pub index: u32,
    pub filename: String,
    pub byte_len: u64,
    pub sha256: String,
    pub oversize: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartSidecar {
    pub schema_version: String,
    pub producer_version: String,
    pub index: u32,
    pub filename: String,
    pub byte_len: u64,
    pub sha256: String,
    pub store_record_key: String,
    pub folder_count: u64,
    pub message_count: u64,
    pub oversize: bool,
    pub partial: bool,
    pub omitted_properties: u64,
    pub omitted_attachments: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerEvent {
    pub kind: String,
    pub attempt: u32,
    pub category: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct JobSourceIdentity {
    pub canonical_path: String,
    pub device: u64,
    pub inode: u64,
    pub size_bytes: u64,
    pub modified_at: String,
    pub sha256: String,
}

pub struct DurableCatalogSink {
    connection: Connection,
    _parent_directory: File,
    _job_directory: File,
    _private_directory: File,
    _spool_directory: File,
    _partial_directory: File,
    _parts_directory: File,
    private_root: PathBuf,
    spool: PathBuf,
    partial: PathBuf,
    parts: PathBuf,
    active: Option<ActiveCandidate>,
    property: Option<ActiveProperty>,
    attachment: Option<ActiveAttachment>,
    unit: Option<RecoveryUnit>,
    recent_candidates: HashMap<Vec<u32>, String>,
}

struct ActiveCandidate {
    key: String,
    message_id: u32,
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
    file: NamedTempFile,
    hasher: Sha256,
    bytes: u64,
}

impl DurableCatalogSink {
    pub fn create(job_directory: &Path) -> Result<Self, JobError> {
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
        for directory in [&private_root, &spool, &partial] {
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
        let partial_directory = open_directory(&private_root.join("partial"))?;
        let partial = fd_path(partial_directory.as_raw_fd());
        let parts_directory = open_directory(&parts)?;
        let parts = fd_path(parts_directory.as_raw_fd());
        let database = private_root.join("job.sqlite3");
        let connection = Connection::open_with_flags(
            &database,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        set_mode(&database, 0o600)?;
        configure(&connection)?;
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
            _parts_directory: parts_directory,
            private_root,
            spool,
            partial,
            parts,
            active: None,
            property: None,
            attachment: None,
            unit: None,
            recent_candidates: HashMap::new(),
        })
    }

    pub fn open(job_directory: &Path) -> Result<Self, JobError> {
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
        let database = private_root.join("job.sqlite3");
        validate_private_file(&database, true)?;
        validate_private_file(&private_root.join("job.sqlite3-wal"), false)?;
        validate_private_file(&private_root.join("job.sqlite3-shm"), false)?;
        let mut connection =
            Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure(&connection)?;
        validate_private_file(&database, true)?;
        validate_private_file(&private_root.join("job.sqlite3-wal"), false)?;
        validate_private_file(&private_root.join("job.sqlite3-shm"), false)?;
        let schema = connection
            .query_row(
                "SELECT value FROM job_metadata WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )?
            .parse::<i64>()
            .map_err(|_| JobError::Integrity("invalid schema version".to_owned()))?;
        if schema != JOB_SCHEMA_VERSION {
            return Err(JobError::Integrity(format!(
                "unsupported schema version {schema}"
            )));
        }
        let integrity = connection.query_row("PRAGMA integrity_check(1)", [], |row| {
            row.get::<_, String>(0)
        })?;
        if integrity != "ok" {
            return Err(JobError::Integrity(integrity));
        }
        reconcile_publications(
            &mut connection,
            &partial_directory,
            &parts_directory,
            &partial,
            &parts,
        )?;
        validate_foreign_keys(&connection)?;
        validate_blob_store(&connection, &spool)?;
        validate_part_store(&connection, &parts)?;
        remove_temporary_blobs(&spool)?;
        remove_unreferenced_blobs(&connection, &spool)?;
        Ok(Self {
            connection,
            _parent_directory: parent_directory,
            _job_directory: job_handle,
            _private_directory: private_directory,
            _spool_directory: spool_directory,
            _partial_directory: partial_directory,
            _parts_directory: parts_directory,
            private_root,
            spool,
            partial,
            parts,
            active: None,
            property: None,
            attachment: None,
            unit: None,
            recent_candidates: HashMap::new(),
        })
    }

    pub fn checkpoint(&self) -> Result<(), JobError> {
        if self.active.is_some() {
            return Err(JobError::EventSequence(
                "cannot checkpoint during an active candidate".to_owned(),
            ));
        }
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        sync_directory(&self.private_root)
    }

    pub fn mark_candidates_unsupported(&mut self, item_keys: &[String]) -> Result<(), JobError> {
        if self.active.is_some() || item_keys.is_empty() {
            return Err(JobError::EventSequence(
                "cannot reject an empty candidate set during an active candidate".to_owned(),
            ));
        }
        let transaction = self.connection.transaction()?;
        for item_key in item_keys {
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
                          'output_unrepresentable', '{}', NULL, NULL \
                   FROM candidate_events WHERE item_key = ?1",
                [item_key],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn abort_worker_attempt(&mut self) -> Result<(), JobError> {
        self.property = None;
        self.attachment = None;
        self.unit = None;
        self.recent_candidates.clear();
        if self.active.take().is_some() {
            self.connection.execute_batch("ROLLBACK")?;
        }
        Ok(())
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
        let (blob_count, blob_bytes) = self.connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(byte_len), 0) FROM blobs",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
        )?;
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
            "SELECT provenance, source_node_id, recovery_index, occurrence, metadata_json, recovery_unit_json \
             FROM candidates \
             WHERE status IN ('spooled', 'written', 'unsupported') ORDER BY rowid",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(provenance, source_node_id, recovery_index, occurrence, metadata, unit)| {
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
            "SELECT source_id, parent_source_id, name, address_json \
                 FROM folders ORDER BY folder_key",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(source_id, parent_source_id, name, address)| {
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
                })
            })
            .collect()
    }

    pub fn spooled_candidates(&self) -> Result<Vec<SpooledCandidate>, JobError> {
        let mut statement = self.connection.prepare(
            "SELECT item_key, provenance, source_node_id, recovery_index, occurrence, \
                    completeness, metadata_json, parent_item_key, parent_attachment_index, \
                    recovery_unit_json \
             FROM candidates WHERE status = 'spooled' \
             ORDER BY provenance, source_node_id, recovery_index, occurrence, item_key",
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
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<String>>(9)?,
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
                    completeness,
                    metadata,
                    parent_item_key,
                    parent_attachment_index,
                    unit,
                )| {
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
                    let events = self.spooled_events(&item_key)?;
                    Ok(SpooledCandidate {
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
                    })
                },
            )
            .collect()
    }

    pub fn candidate_ownerships(&self) -> Result<Vec<CandidateOwnership>, JobError> {
        let mut statement = self.connection.prepare(
            "SELECT item_key, source_node_id, status, parent_item_key, \
                    parent_attachment_index, embedded_path_json, metadata_json \
             FROM candidates WHERE status IN ('spooled', 'unsupported') ORDER BY item_key",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
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
        if blob.sha256.len() != 64
            || !blob
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(JobError::Integrity("invalid spool blob digest".to_owned()));
        }
        let expected = self.connection.query_row(
            "SELECT byte_len FROM blobs WHERE sha256 = ?1",
            [&blob.sha256],
            |row| row.get::<_, u64>(0),
        )?;
        if expected != blob.byte_len {
            return Err(JobError::Integrity(format!(
                "spool blob {} length disagrees with the ledger",
                blob.sha256
            )));
        }
        open_verified_blob(&self.spool.join(&blob.sha256), &blob.sha256, blob.byte_len)
    }

    /// Return a path rooted at the held private spool directory after verifying
    /// that the ledger entry and immutable blob still agree.
    pub fn verified_blob_path(&self, blob: &SpooledBlob) -> Result<PathBuf, JobError> {
        drop(self.open_blob(blob)?);
        Ok(self.spool.join(&blob.sha256))
    }

    pub fn staged_part_path(&self, filename: &str) -> Result<PathBuf, JobError> {
        if !valid_leaf_name(filename) || !filename.ends_with(".partial") {
            return Err(JobError::UnsafePath(PathBuf::from(filename)));
        }
        Ok(self.partial.join(filename))
    }

    pub fn publish_validated_part(
        &mut self,
        staged_filename: &str,
        part: &PublishedPart,
        sidecar: &PartSidecar,
        item_keys: &[String],
    ) -> Result<(), JobError> {
        validate_part_record(part)?;
        validate_sidecar(part, sidecar)?;
        let staged = self.staged_part_path(staged_filename)?;
        verify_part_artifact(&staged, part)?;
        let final_path = self.parts.join(&part.filename);
        let sidecar_filename = part.filename.trim_end_matches(".pst").to_owned() + ".json";
        let sidecar_path = self.parts.join(&sidecar_filename);
        let staged_sidecar_filename = sidecar_filename.clone() + ".partial";
        let staged_sidecar_path = self.partial.join(&staged_sidecar_filename);
        refuse_existing(&final_path)?;
        refuse_existing(&sidecar_path)?;
        refuse_existing(&staged_sidecar_path)?;
        write_sidecar_partial(&staged_sidecar_path, sidecar)?;

        self.record_publication_intent(part, sidecar, item_keys)?;

        rename_noclobber(
            &self._partial_directory,
            Path::new(staged_filename),
            &self._parts_directory,
            Path::new(&part.filename),
            &final_path,
        )?;
        sync_file(&self._parts_directory, &self.parts)?;
        verify_part_artifact(&final_path, part)?;
        rename_noclobber(
            &self._partial_directory,
            Path::new(&staged_sidecar_filename),
            &self._parts_directory,
            Path::new(&sidecar_filename),
            &sidecar_path,
        )?;
        sync_file(&self._parts_directory, &self.parts)?;
        verify_sidecar_artifact(&sidecar_path, sidecar)?;
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
        validate_part_record(part)?;
        validate_sidecar(part, sidecar)?;
        verify_part_artifact(&self.parts.join(&part.filename), part)?;
        commit_published_part_transaction(&mut self.connection, part, sidecar, item_keys, false)
    }

    fn spooled_events(&self, item_key: &str) -> Result<Vec<SpooledEvent>, JobError> {
        let mut statement = self.connection.prepare(
            "SELECT sequence, kind, metadata_json, blob_sha256, byte_len \
             FROM candidate_events WHERE item_key = ?1 ORDER BY sequence",
        )?;
        let rows = statement
            .query_map([item_key], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(sequence, kind, metadata, sha256, byte_len)| {
                let blob = match (sha256, byte_len) {
                    (Some(sha256), Some(byte_len)) => Some(SpooledBlob {
                        sha256,
                        byte_len: checked_u64(byte_len, "candidate event byte length")?,
                    }),
                    (None, None) => None,
                    _ => {
                        return Err(JobError::Integrity(
                            "candidate event has an incomplete blob reference".to_owned(),
                        ));
                    }
                };
                Ok(SpooledEvent {
                    sequence: checked_u64(sequence, "candidate event sequence")?,
                    kind,
                    metadata: serde_json::from_str(&metadata)?,
                    blob,
                })
            })
            .collect()
    }

    fn start_candidate(&mut self, start: CandidateStart) -> Result<(), JobError> {
        let CandidateStart {
            metadata,
            id,
            provenance,
            recovery_index,
            parent_message_id,
            parent_attachment_index,
            embedded_path,
            supported,
        } = start;
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
        if parent_item_key.is_some() != parent_attachment_index.is_some() {
            return Err(JobError::EventSequence(
                "embedded message parent attachment is incomplete".to_owned(),
            ));
        }
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
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
                serde_json::to_string(&metadata)?,
                self.unit
                    .map(|unit| serde_json::to_string(&unit))
                    .transpose()?,
                parent_item_key,
                parent_attachment_index,
                serde_json::to_string(&embedded_path)?,
            ],
        );
        if let Err(error) = result {
            let _ = self.connection.execute_batch("ROLLBACK");
            return Err(error.into());
        }
        self.active = Some(ActiveCandidate {
            key,
            message_id: id,
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
        self.connection.execute(
            "INSERT INTO candidate_events(\
                item_key, sequence, kind, metadata_json, blob_sha256, byte_len\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                active.key,
                active.sequence,
                kind,
                serde_json::to_string(&metadata)?,
                blob.as_ref().map(|value| value.sha256.as_str()),
                blob.as_ref().map(|value| value.bytes),
            ],
        )?;
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
            let blob = self.finish_blob(active.blob, Some(descriptor.data_size))?;
            self.record_event("property", property_json(descriptor), Some(blob))
        } else {
            discard_blob(active.blob, descriptor.data_size)
        }
    }

    fn finish_attachment(&mut self, complete: bool) -> Result<(), JobError> {
        let Some(active) = self.attachment.take() else {
            return Ok(());
        };
        if let Some(blob) = active.blob {
            let actual = blob.bytes;
            let blob = self.finish_blob(blob, complete.then_some(active.expected).flatten())?;
            self.record_event(
                if complete {
                    "attachment_data"
                } else {
                    "attachment_partial"
                },
                json!({
                    "message_id": active.message_id,
                    "index": active.index,
                    "declared_size": active.expected,
                    "actual_size": actual,
                }),
                Some(blob),
            )?;
        } else if complete && active.attachment_type == Some(i32::from(b'i')) {
            return Ok(());
        } else if complete && active.expected == Some(0) {
            let blob = BlobWriter::new(&self.spool)?;
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
        mut blob: BlobWriter,
        expected: Option<u64>,
    ) -> Result<BlobRef, JobError> {
        blob.file
            .flush()
            .map_err(|source| io_error(blob.file.path(), source))?;
        blob.file
            .as_file()
            .sync_all()
            .map_err(|source| io_error(blob.file.path(), source))?;
        if let Some(expected) = expected {
            if expected != blob.bytes {
                return Err(JobError::BlobLength {
                    expected,
                    actual: blob.bytes,
                });
            }
        }
        let sha256 = digest_hex(blob.hasher.finalize().as_slice());
        let destination = self.spool.join(&sha256);
        match destination.symlink_metadata() {
            Ok(_) => verify_blob(&destination, &sha256, blob.bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                blob.file
                    .persist_noclobber(&destination)
                    .map_err(|error| io_error(&destination, error.error))?;
                sync_directory(&self.spool)?;
            }
            Err(source) => return Err(io_error(&destination, source)),
        }
        verify_blob(&destination, &sha256, blob.bytes)?;
        self.connection.execute(
            "INSERT OR IGNORE INTO blobs(sha256, byte_len) VALUES (?1, ?2)",
            params![sha256, blob.bytes],
        )?;
        Ok(BlobRef {
            sha256,
            bytes: blob.bytes,
        })
    }

    fn rollback(&mut self) {
        self.property = None;
        self.attachment = None;
        self.unit = None;
        self.recent_candidates.clear();
        if self.active.take().is_some() {
            let _ = self.connection.execute_batch("ROLLBACK");
        }
    }
}

impl CatalogSink for DurableCatalogSink {
    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
        self.handle_event(event).map_err(|error| error.to_string())
    }
}

impl DurableCatalogSink {
    fn handle_event(&mut self, event: CatalogEvent<'_>) -> Result<(), JobError> {
        match event {
            CatalogEvent::UnitStart(unit) => {
                if self.active.is_some() {
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
            }
            CatalogEvent::UnitEnd(unit) => {
                if self.active.is_some() || self.unit.take() != Some(unit) {
                    return Err(JobError::EventSequence(
                        "recovery unit ended out of sequence".to_owned(),
                    ));
                }
            }
            CatalogEvent::Folder {
                id,
                parent_id,
                name,
            } => {
                if self.active.is_some() {
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
                        folder_key, source_id, parent_source_id, name, address_json\
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        folder_key,
                        i64::from(id),
                        parent_id.map(i64::from),
                        name,
                        address_json
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
                let attachment = self.attachment.as_mut().ok_or_else(|| {
                    JobError::EventSequence("attachment data without metadata".to_owned())
                })?;
                if attachment.message_id != message_id || attachment.index != index {
                    return Err(JobError::EventSequence(
                        "attachment data does not match active attachment".to_owned(),
                    ));
                }
                if attachment.blob.is_none() {
                    attachment.blob = Some(BlobWriter::new(&self.spool)?);
                }
                attachment
                    .blob
                    .as_mut()
                    .ok_or_else(|| JobError::EventSequence("attachment blob missing".to_owned()))?
                    .write(bytes)?;
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
                self.property = Some(ActiveProperty {
                    descriptor,
                    blob: BlobWriter::new(&self.spool)?,
                    record: !matches!(descriptor.owner, PropertyOwner::Folder(_)),
                });
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
                property.blob.write(bytes)?;
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
                if property.record {
                    self.record_event(
                        "property_incomplete",
                        json!({
                            "property": property_json(property.descriptor),
                            "reason": reason,
                        }),
                        None,
                    )?;
                }
            }
            CatalogEvent::MessageEnd { id, complete } => {
                require_message(self.active.as_ref(), id)?;
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
                        self.record_event(
                            "property_incomplete",
                            json!({
                                "property": property_json(property.descriptor),
                                "reason": "message ended before property completion",
                            }),
                            None,
                        )?;
                    }
                }
                if !complete {
                    self.finish_attachment(false)?;
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
                if let Err(error) = self.connection.execute_batch("COMMIT") {
                    let _ = self.connection.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                self.recent_candidates
                    .insert(active.embedded_path, active.key.clone());
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
    fn new(spool: &Path) -> Result<Self, JobError> {
        let file = NamedTempFile::new_in(spool).map_err(|source| io_error(spool, source))?;
        Ok(Self {
            file,
            hasher: Sha256::new(),
            bytes: 0,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), JobError> {
        write_hashed(&mut self.file, &mut self.hasher, &mut self.bytes, bytes)
            .map_err(|source| io_error(self.file.path(), source))
    }
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
            address_json TEXT\
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
            byte_len INTEGER NOT NULL CHECK(byte_len >= 0)\
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
            sha256 TEXT NOT NULL CHECK(\
                length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'\
            ),\
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

fn property_json(descriptor: PropertyDescriptor) -> serde_json::Value {
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
    parts_directory: &File,
    partial: &Path,
    parts: &Path,
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
        let sidecar_path = parts.join(&sidecar_filename);
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
        verify_part_artifact(&final_path, &part)?;
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
                parts_directory,
                Path::new(&sidecar_filename),
                &sidecar_path,
            )?;
            sync_file(parts_directory, parts)?;
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
        || part.sha256.len() != 64
        || !part
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(JobError::Integrity(
            "invalid published part accounting record".to_owned(),
        ));
    }
    Ok(())
}

fn validate_sidecar(part: &PublishedPart, sidecar: &PartSidecar) -> Result<(), JobError> {
    if sidecar.schema_version != "1.0.0"
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

fn validate_blob_store(connection: &Connection, spool: &Path) -> Result<(), JobError> {
    let mut statement = connection.prepare("SELECT sha256, byte_len FROM blobs ORDER BY sha256")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let sha256 = row.get::<_, String>(0)?;
        let byte_len = row.get::<_, u64>(1)?;
        if !valid_blob_hash(&sha256) {
            return Err(JobError::Integrity(
                "blob key is not lowercase SHA-256".to_owned(),
            ));
        }
        verify_blob(&spool.join(&sha256), &sha256, byte_len)?;
    }
    Ok(())
}

fn validate_part_store(connection: &Connection, parts: &Path) -> Result<(), JobError> {
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
    let mut accounted = HashSet::new();
    let mut accounted_sidecars = HashSet::new();
    for (part, sidecar_json) in rows {
        validate_part_record(&part)?;
        verify_part_artifact(&parts.join(&part.filename), &part)?;
        let sidecar_filename = part.filename.trim_end_matches(".pst").to_owned() + ".json";
        let expected_sidecar: PartSidecar = serde_json::from_str(&sidecar_json)?;
        validate_sidecar(&part, &expected_sidecar)?;
        verify_sidecar_artifact(&parts.join(&sidecar_filename), &expected_sidecar)?;
        accounted_sidecars.insert(sidecar_filename);
        accounted.insert(part.filename);
    }
    for entry in fs::read_dir(parts).map_err(|source| io_error(parts, source))? {
        let entry = entry.map_err(|source| io_error(parts, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".pst") && !accounted.contains(name.as_ref()) {
            return Err(JobError::Integrity(format!(
                "finalized PST {name:?} has no ledger record"
            )));
        } else if name.ends_with(".json") && !accounted_sidecars.contains(name.as_ref()) {
            return Err(JobError::Integrity(format!(
                "finalized sidecar {name:?} has no ledger record"
            )));
        }
    }
    Ok(())
}

fn verify_part_artifact(path: &Path, part: &PublishedPart) -> Result<(), JobError> {
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
    {
        return Err(JobError::Integrity(format!(
            "published part {} changed while opening",
            part.filename
        )));
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| io_error(path, source))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if digest_hex(hasher.finalize().as_slice()) != part.sha256 {
        return Err(JobError::Integrity(format!(
            "published part {} failed SHA-256 validation",
            part.filename
        )));
    }
    Ok(())
}

fn valid_blob_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn verify_blob(path: &Path, expected_hash: &str, expected_len: u64) -> Result<(), JobError> {
    open_verified_blob(path, expected_hash, expected_len).map(|_| ())
}

fn open_verified_blob(
    path: &Path,
    expected_hash: &str,
    expected_len: u64,
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

fn remove_temporary_blobs(spool: &Path) -> Result<(), JobError> {
    for entry in fs::read_dir(spool).map_err(|source| io_error(spool, source))? {
        let entry = entry.map_err(|source| io_error(spool, source))?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".tmp") {
            fs::remove_file(entry.path()).map_err(|source| io_error(&entry.path(), source))?;
        }
    }
    Ok(())
}

fn remove_unreferenced_blobs(connection: &Connection, spool: &Path) -> Result<(), JobError> {
    for entry in fs::read_dir(spool).map_err(|source| io_error(spool, source))? {
        let entry = entry.map_err(|source| io_error(spool, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !valid_blob_hash(&name) {
            continue;
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

fn discard_blob(blob: BlobWriter, expected: u64) -> Result<(), JobError> {
    if blob.bytes != expected {
        return Err(JobError::BlobLength {
            expected,
            actual: blob.bytes,
        });
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
    use std::io::Write as _;
    use std::io::{BufRead, BufReader, Read as _};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use libpff_sys::{
        CatalogEvent, CatalogProvenance, CatalogSink, PropertyDescriptor, PropertyOwner,
    };
    use rusqlite::Connection;
    use tempfile::tempdir;

    use sha2::{Digest, Sha256};

    use super::{
        DurableCatalogSink, JobError, PartSidecar, PublishedPart, WorkerEvent, digest_hex,
        private_state_attributes_valid, write_hashed, write_sidecar_partial,
    };

    struct PrefixThenError {
        remaining: usize,
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
        Ok(())
    }

    #[test]
    fn worker_supervision_events_survive_reopen() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job)?;
        sink.record_worker_event("started", 1, "parser")?;
        sink.record_worker_event("failure", 1, "stall")?;
        drop(sink);
        let reopened = DurableCatalogSink::open(&job)?;
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
    fn spool_read_model_and_part_assignment_are_transactional()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        sink.event(CatalogEvent::Folder {
            id: 1,
            parent_id: None,
            name: Some("private folder".to_owned()),
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
        assert!(matches!(sink.open_blob(blob), Err(JobError::Integrity(_))));
        std::fs::write(&blob_path, b"body")?;

        let part_bytes = vec![0_u8; 1024];
        let part_path = job.join("parts/part-0001.pst");
        std::fs::write(&part_path, &part_bytes)?;
        std::fs::set_permissions(&part_path, std::fs::Permissions::from_mode(0o600))?;
        let part = PublishedPart {
            index: 1,
            filename: "part-0001.pst".to_owned(),
            byte_len: 1024,
            sha256: digest_hex(Sha256::digest(&part_bytes).as_slice()),
            oversize: false,
        };
        let sidecar = PartSidecar {
            schema_version: "1.0.0".to_owned(),
            producer_version: "0.4.0".to_owned(),
            index: part.index,
            filename: part.filename.clone(),
            byte_len: part.byte_len,
            sha256: part.sha256.clone(),
            store_record_key: "0".repeat(32),
            folder_count: 1,
            message_count: 1,
            oversize: part.oversize,
            partial: false,
            omitted_properties: 0,
            omitted_attachments: 0,
        };
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
        let sidecar_path = job.join("parts/part-0001.json");
        std::fs::write(&sidecar_path, serde_json::to_vec(&sidecar)?)?;
        std::fs::set_permissions(&sidecar_path, std::fs::Permissions::from_mode(0o600))?;
        assert!(sink.spooled_candidates()?.is_empty());
        assert_eq!(sink.summary()?.committed_candidates, 1);
        assert!(
            sink.commit_published_part(&part, &sidecar, &["normal:10:-:0".to_owned()],)
                .is_err()
        );
        sink.checkpoint()?;
        drop(sink);
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
            let item_key = sink.spooled_candidates()?[0].item_key.clone();
            let bytes = b"validated part checkpoint";
            let part = PublishedPart {
                index: 1,
                filename: "part-0001.pst".to_owned(),
                byte_len: bytes.len() as u64,
                sha256: digest_hex(Sha256::digest(bytes).as_slice()),
                oversize: false,
            };
            let sidecar = PartSidecar {
                schema_version: "1.0.0".to_owned(),
                producer_version: "0.4.0".to_owned(),
                index: 1,
                filename: part.filename.clone(),
                byte_len: part.byte_len,
                sha256: part.sha256.clone(),
                store_record_key: "0".repeat(32),
                folder_count: 1,
                message_count: 1,
                oversize: false,
                partial: false,
                omitted_properties: 0,
                omitted_attachments: 0,
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
                    job.join("parts/part-0001.json"),
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
                    job.join(".pstforge/partial/part-0001.json.partial")
                        .is_file()
                );
            } else {
                assert!(reopened.spooled_candidates()?.is_empty());
                assert!(job.join("parts/part-0001.pst").is_file());
                assert!(job.join("parts/part-0001.json").is_file());
            }
        }
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
        assert_eq!(std::fs::read_dir(job.join(".pstforge/spool"))?.count(), 0);
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
        let spool = job.join(".pstforge/spool");
        let blob = std::fs::read_dir(&spool)?
            .next()
            .ok_or("missing committed blob")??
            .path();
        std::fs::write(blob, b"evil")?;

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
        let foreign = effective.checked_add(1).unwrap_or(effective - 1);
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
        assert_eq!(std::fs::read_dir(job.join(".pstforge/spool"))?.count(), 0);

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
            let blob = std::fs::read_dir(job.join(".pstforge/spool"))?
                .next()
                .ok_or("missing blob")??
                .path();
            if hard_link {
                std::fs::hard_link(&blob, directory.path().join("outside-blob"))?;
            } else {
                std::fs::set_permissions(&blob, std::fs::Permissions::from_mode(0o644))?;
            }
            assert!(matches!(
                DurableCatalogSink::open(&job),
                Err(JobError::Integrity(_))
            ));
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
}
