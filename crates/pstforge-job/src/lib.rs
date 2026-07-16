#![deny(unsafe_code)]

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write as _};
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

const JOB_SCHEMA_VERSION: i64 = 3;

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
    private_root: PathBuf,
    spool: PathBuf,
    active: Option<ActiveCandidate>,
    property: Option<ActiveProperty>,
    attachment: Option<ActiveAttachment>,
    unit: Option<RecoveryUnit>,
}

struct ActiveCandidate {
    key: String,
    message_id: u32,
    sequence: u64,
    recipients: HashSet<u32>,
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
        let private_directory = open_directory(&private_root)?;
        let private_root = fd_path(private_directory.as_raw_fd());
        let spool_directory = open_directory(&private_root.join("spool"))?;
        let spool = fd_path(spool_directory.as_raw_fd());
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
            private_root,
            spool,
            active: None,
            property: None,
            attachment: None,
            unit: None,
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
        let partial = open_directory(&private_root.join("partial"))?;
        validate_private_directory(&partial, &private_root.join("partial"))?;
        let database = private_root.join("job.sqlite3");
        validate_private_file(&database, true)?;
        validate_private_file(&private_root.join("job.sqlite3-wal"), false)?;
        validate_private_file(&private_root.join("job.sqlite3-shm"), false)?;
        let connection = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
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
        validate_foreign_keys(&connection)?;
        validate_blob_store(&connection, &spool)?;
        remove_temporary_blobs(&spool)?;
        remove_unreferenced_blobs(&connection, &spool)?;
        Ok(Self {
            connection,
            _parent_directory: parent_directory,
            _job_directory: job_handle,
            _private_directory: private_directory,
            _spool_directory: spool_directory,
            private_root,
            spool,
            active: None,
            property: None,
            attachment: None,
            unit: None,
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

    pub fn abort_worker_attempt(&mut self) -> Result<(), JobError> {
        self.property = None;
        self.attachment = None;
        self.unit = None;
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
             FROM candidates WHERE status IN ('spooled', 'unsupported')",
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
             WHERE status IN ('spooled', 'unsupported') ORDER BY rowid",
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

    fn start_candidate(
        &mut self,
        metadata: serde_json::Value,
        id: u32,
        provenance: CatalogProvenance,
        recovery_index: Option<u64>,
        supported: bool,
    ) -> Result<(), JobError> {
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
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let result = self.connection.execute(
            "INSERT INTO candidates(\
                item_key, provenance, source_node_id, recovery_index, occurrence,\
                completeness, status, metadata_json, recovery_unit_json\
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'damaged', 'pending', ?6, ?7)",
            params![
                key,
                provenance,
                source_node_id,
                recovery_index,
                occurrence,
                serde_json::to_string(&metadata)?,
                self.unit
                    .map(|unit| serde_json::to_string(&unit))
                    .transpose()?
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
        } else if complete && active.expected.is_some_and(|size| size != 0) {
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
                self.connection.execute(
                    "INSERT OR REPLACE INTO folders(source_id, parent_source_id, name) VALUES (?1, ?2, ?3)",
                    params![i64::from(id), parent_id.map(i64::from), name],
                )?;
            }
            CatalogEvent::MessageStart {
                id,
                provenance,
                recovery_index,
                folder_id,
                parent_message_id,
                parent_attachment_index,
                item_type,
                message_class,
                subject,
                sender_name,
                sender_email,
                submit_filetime,
                delivery_filetime,
                supported,
            } => self.start_candidate(
                json!({
                    "folder_id": folder_id,
                    "parent_message_id": parent_message_id,
                    "parent_attachment_index": parent_attachment_index,
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
                supported,
            )?,
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
            source_id INTEGER PRIMARY KEY,\
            parent_source_id INTEGER,\
            name TEXT\
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
            recovery_unit_json TEXT\
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

fn valid_blob_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn verify_blob(path: &Path, expected_hash: &str, expected_len: u64) -> Result<(), JobError> {
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
    Ok(())
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
    use std::io::{BufRead, BufReader};
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
        DurableCatalogSink, JobError, WorkerEvent, digest_hex, private_state_attributes_valid,
        write_hashed,
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
                    sink.event(CatalogEvent::AttachmentEnd {
                        message_id: 10,
                        index: 0,
                    })?;
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
