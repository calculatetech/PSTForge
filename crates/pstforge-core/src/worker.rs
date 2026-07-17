use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::path::Path;

use libpff_sys::{
    CatalogEvent, CatalogProvenance, CatalogSink, PffError, PffFile, PropertyDescriptor,
    RawCatalog, RecoveryMode, RecoveryUnit, STREAM_CHUNK_BYTES,
};
use pstforge_job::ReplayCandidate;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::{SourceError, SourceFile};

const PROTOCOL_VERSION: u32 = 1;
const MAX_CONTROL_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum WorkerProtocolError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Pff(#[from] PffError),
    #[error("worker protocol I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("worker protocol JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid worker protocol: {0}")]
    Invalid(String),
    #[error("catalog sink rejected worker event: {0}")]
    Sink(String),
    #[error("worker rejected the source: {0}")]
    ReportedSource(String),
    #[error("worker parsing failed: {0}")]
    ReportedParser(String),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlFrame {
    Hello {
        version: u32,
    },
    Error {
        kind: WorkerFailureKind,
        detail: String,
    },
    UnitStart {
        unit: RecoveryUnit,
    },
    UnitEnd {
        unit: RecoveryUnit,
    },
    Folder {
        id: u32,
        parent_id: Option<u32>,
        name: Option<String>,
    },
    MessageStart {
        id: u32,
        provenance: CatalogProvenance,
        recovery_index: Option<u64>,
        folder_id: Option<u32>,
        parent_message_id: Option<u32>,
        parent_attachment_index: Option<u32>,
        embedded_path: Vec<u32>,
        item_type: Option<u8>,
        message_class: Option<String>,
        subject: Option<String>,
        sender_name: Option<String>,
        sender_email: Option<String>,
        submit_filetime: Option<u64>,
        delivery_filetime: Option<u64>,
        supported: bool,
    },
    Recipient {
        message_id: u32,
        index: u32,
        recipient_type: Option<u32>,
        display_name: Option<String>,
        email_address: Option<String>,
        address_type: Option<String>,
    },
    AttachmentStart {
        message_id: u32,
        index: u32,
        attachment_type: Option<i32>,
        data_size: Option<u64>,
        filename: Option<String>,
    },
    AttachmentData {
        message_id: u32,
        index: u32,
        byte_len: u32,
    },
    AttachmentEnd {
        message_id: u32,
        index: u32,
    },
    AttachmentAbort {
        message_id: u32,
        index: u32,
    },
    PropertyStart {
        descriptor: PropertyDescriptor,
    },
    PropertyData {
        descriptor: PropertyDescriptor,
        byte_len: u32,
    },
    PropertyEnd {
        descriptor: PropertyDescriptor,
    },
    PropertyAbort {
        descriptor: PropertyDescriptor,
        reason: String,
    },
    MessageEnd {
        id: u32,
        complete: bool,
    },
    Complete {
        catalog: WorkerCatalog,
    },
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct WorkerCatalog {
    pub(crate) folders: u64,
    pub(crate) messages: u64,
    pub(crate) recovered_messages: u64,
    pub(crate) orphan_messages: u64,
    pub(crate) fragment_messages: u64,
    pub(crate) recipients: u64,
    pub(crate) attachments: u64,
    pub(crate) embedded_messages: u64,
    pub(crate) unsupported_messages: u64,
    pub(crate) properties: u64,
    pub(crate) property_bytes: u64,
    pub(crate) attachment_bytes: u64,
    pub(crate) issues: u64,
    pub(crate) issues_dropped: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkerFailureKind {
    Source,
    Parser,
}

impl From<RawCatalog> for WorkerCatalog {
    fn from(catalog: RawCatalog) -> Self {
        Self {
            folders: catalog.folders,
            messages: catalog.messages,
            recovered_messages: catalog.recovered_messages,
            orphan_messages: catalog.orphan_messages,
            fragment_messages: catalog.fragment_messages,
            recipients: catalog.recipients,
            attachments: catalog.attachments,
            embedded_messages: catalog.embedded_messages,
            unsupported_messages: catalog.unsupported_messages,
            properties: catalog.properties,
            property_bytes: catalog.property_bytes,
            attachment_bytes: catalog.attachment_bytes,
            issues: u64::try_from(catalog.issues.len()).unwrap_or(u64::MAX),
            issues_dropped: catalog.issues_dropped,
        }
    }
}

struct ProtocolSink<'a> {
    output: &'a mut dyn Write,
    completed_candidates: u64,
    abort_after_candidates: Option<u64>,
    stall_after_candidates: Option<u64>,
    units_started: u64,
    abort_on_unit: Option<u64>,
    abort_unit: Option<RecoveryUnit>,
    abort_inside_unit: Option<RecoveryUnit>,
    abort_inside_after_candidates: Option<u64>,
    segv_on_unit: Option<u64>,
    segv_unit: Option<RecoveryUnit>,
    parser_error_after_candidates: Option<u64>,
    active_unit: Option<RecoveryUnit>,
    abort_at_unit_end: bool,
    stall_at_unit_end: bool,
    parser_error_at_unit_end: bool,
}

impl ProtocolSink<'_> {
    fn start(output: &mut dyn Write) -> Result<ProtocolSink<'_>, WorkerProtocolError> {
        write_control(
            output,
            &ControlFrame::Hello {
                version: PROTOCOL_VERSION,
            },
        )?;
        let abort_after_candidates = std::env::var("PSTFORGE_INTERNAL_ABORT_AFTER_CANDIDATES")
            .ok()
            .and_then(|value| value.parse().ok());
        let stall_after_candidates = std::env::var("PSTFORGE_INTERNAL_STALL_AFTER_CANDIDATES")
            .ok()
            .and_then(|value| value.parse().ok());
        let abort_on_unit = std::env::var("PSTFORGE_INTERNAL_ABORT_ON_UNIT")
            .ok()
            .and_then(|value| value.parse().ok());
        let abort_unit = std::env::var("PSTFORGE_INTERNAL_ABORT_UNIT")
            .ok()
            .and_then(|value| serde_json::from_str(&value).ok());
        let abort_inside_unit = std::env::var("PSTFORGE_INTERNAL_ABORT_INSIDE_UNIT")
            .ok()
            .and_then(|value| serde_json::from_str(&value).ok());
        let abort_inside_after_candidates =
            std::env::var("PSTFORGE_INTERNAL_ABORT_INSIDE_AFTER_CANDIDATES")
                .ok()
                .and_then(|value| value.parse().ok());
        let segv_on_unit = std::env::var("PSTFORGE_INTERNAL_SEGV_ON_UNIT")
            .ok()
            .and_then(|value| value.parse().ok());
        let segv_unit = std::env::var("PSTFORGE_INTERNAL_SEGV_UNIT")
            .ok()
            .and_then(|value| serde_json::from_str(&value).ok());
        let parser_error_after_candidates =
            std::env::var("PSTFORGE_INTERNAL_PARSER_ERROR_AFTER_CANDIDATES")
                .ok()
                .and_then(|value| value.parse().ok());
        Ok(ProtocolSink {
            output,
            completed_candidates: 0,
            abort_after_candidates,
            stall_after_candidates,
            units_started: 0,
            abort_on_unit,
            abort_unit,
            abort_inside_unit,
            abort_inside_after_candidates,
            segv_on_unit,
            segv_unit,
            parser_error_after_candidates,
            active_unit: None,
            abort_at_unit_end: false,
            stall_at_unit_end: false,
            parser_error_at_unit_end: false,
        })
    }

    fn complete(&mut self, catalog: RawCatalog) -> Result<(), WorkerProtocolError> {
        write_control(
            self.output,
            &ControlFrame::Complete {
                catalog: catalog.into(),
            },
        )?;
        self.output.flush()?;
        Ok(())
    }

    fn send(&mut self, frame: &ControlFrame) -> Result<(), String> {
        write_control(self.output, frame).map_err(|error| error.to_string())
    }

    fn send_data(&mut self, frame: &ControlFrame, bytes: &[u8]) -> Result<(), String> {
        self.send(frame)?;
        self.output
            .write_all(bytes)
            .map_err(|error| WorkerProtocolError::Io(error).to_string())
    }
}

impl CatalogSink for ProtocolSink<'_> {
    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
        match event {
            CatalogEvent::UnitStart(unit) => {
                self.send(&ControlFrame::UnitStart { unit })?;
                self.active_unit = Some(unit);
                self.units_started = self.units_started.saturating_add(1);
                if self.abort_on_unit == Some(self.units_started) || self.abort_unit == Some(unit) {
                    let _ = self.output.flush();
                    std::process::abort();
                }
                if self.segv_on_unit == Some(self.units_started) || self.segv_unit == Some(unit) {
                    let _ = self.output.flush();
                    let _ = rustix::process::kill_process(
                        rustix::process::getpid(),
                        rustix::process::Signal::SEGV,
                    );
                    std::process::abort();
                }
                Ok(())
            }
            CatalogEvent::UnitEnd(unit) => {
                self.send(&ControlFrame::UnitEnd { unit })?;
                self.active_unit = None;
                if self.abort_at_unit_end {
                    let _ = self.output.flush();
                    std::process::abort();
                }
                if self.stall_at_unit_end {
                    let _ = self.output.flush();
                    loop {
                        std::thread::park();
                    }
                }
                if self.parser_error_at_unit_end {
                    return Err("injected parser error after committed candidates".to_owned());
                }
                Ok(())
            }
            CatalogEvent::Folder {
                id,
                parent_id,
                name,
            } => self.send(&ControlFrame::Folder {
                id,
                parent_id,
                name,
            }),
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
            } => self.send(&ControlFrame::MessageStart {
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
            }),
            CatalogEvent::Recipient {
                message_id,
                index,
                recipient_type,
                display_name,
                email_address,
                address_type,
            } => self.send(&ControlFrame::Recipient {
                message_id,
                index,
                recipient_type,
                display_name,
                email_address,
                address_type,
            }),
            CatalogEvent::AttachmentStart {
                message_id,
                index,
                attachment_type,
                data_size,
                filename,
            } => self.send(&ControlFrame::AttachmentStart {
                message_id,
                index,
                attachment_type,
                data_size,
                filename,
            }),
            CatalogEvent::AttachmentData {
                message_id,
                index,
                bytes,
            } => {
                let byte_len = u32::try_from(bytes.len())
                    .map_err(|_| "attachment frame length exceeds u32".to_owned())?;
                self.send_data(
                    &ControlFrame::AttachmentData {
                        message_id,
                        index,
                        byte_len,
                    },
                    bytes,
                )
            }
            CatalogEvent::AttachmentEnd { message_id, index } => {
                self.send(&ControlFrame::AttachmentEnd { message_id, index })
            }
            CatalogEvent::AttachmentAbort { message_id, index } => {
                self.send(&ControlFrame::AttachmentAbort { message_id, index })
            }
            CatalogEvent::PropertyStart(descriptor) => {
                self.send(&ControlFrame::PropertyStart { descriptor })
            }
            CatalogEvent::PropertyData { descriptor, bytes } => {
                let byte_len = u32::try_from(bytes.len())
                    .map_err(|_| "property frame length exceeds u32".to_owned())?;
                self.send_data(
                    &ControlFrame::PropertyData {
                        descriptor,
                        byte_len,
                    },
                    bytes,
                )
            }
            CatalogEvent::PropertyEnd(descriptor) => {
                self.send(&ControlFrame::PropertyEnd { descriptor })
            }
            CatalogEvent::PropertyAbort { descriptor, reason } => {
                self.send(&ControlFrame::PropertyAbort { descriptor, reason })
            }
            CatalogEvent::MessageEnd { id, complete } => {
                self.send(&ControlFrame::MessageEnd { id, complete })?;
                self.completed_candidates = self.completed_candidates.saturating_add(1);
                if self.abort_inside_after_candidates == Some(self.completed_candidates)
                    || self
                        .abort_inside_unit
                        .is_some_and(|unit| self.active_unit == Some(unit))
                {
                    let _ = self.output.flush();
                    std::process::abort();
                }
                if self.abort_after_candidates == Some(self.completed_candidates) {
                    self.abort_at_unit_end = true;
                }
                if self.stall_after_candidates == Some(self.completed_candidates) {
                    self.stall_at_unit_end = true;
                }
                if self.parser_error_after_candidates == Some(self.completed_candidates) {
                    self.parser_error_at_unit_end = true;
                }
                Ok(())
            }
        }
    }
}

pub fn run_recovery_worker(
    source_path: &Path,
    expected_identity: &crate::SourceIdentity,
    skipped_units: &std::collections::HashSet<RecoveryUnit>,
    mode: RecoveryMode,
    output: &mut dyn Write,
) -> Result<(), WorkerProtocolError> {
    let source = match SourceFile::open(source_path) {
        Ok(source) => source,
        Err(error) => return report_worker_error(output, WorkerFailureKind::Source, error.into()),
    };
    if source.identity() != expected_identity {
        return report_worker_error(
            output,
            WorkerFailureKind::Source,
            WorkerProtocolError::Invalid(
                "source identity does not match supervisor identity".to_owned(),
            ),
        );
    }
    let mut file = match PffFile::open_fd(source.file().as_fd()) {
        Ok(file) => file,
        Err(error) => return report_worker_error(output, WorkerFailureKind::Source, error.into()),
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => return report_worker_error(output, WorkerFailureKind::Source, error.into()),
    };
    if metadata.content_type != Some(b'p') || metadata.size != source.identity().size_bytes {
        return report_worker_error(
            output,
            WorkerFailureKind::Source,
            WorkerProtocolError::Invalid(
                "worker source metadata does not match an open PST".to_owned(),
            ),
        );
    }
    let mut sink = ProtocolSink::start(output)?;
    let catalog = match file.recovery_catalog_skipping(&mut sink, skipped_units, mode) {
        Ok(catalog) => catalog,
        Err(error) => {
            return report_worker_error(sink.output, WorkerFailureKind::Parser, error.into());
        }
    };
    if let Err(error) = source.verify_unchanged() {
        return report_worker_error(sink.output, WorkerFailureKind::Source, error.into());
    }
    sink.complete(catalog)
}

fn report_worker_error(
    output: &mut dyn Write,
    kind: WorkerFailureKind,
    error: WorkerProtocolError,
) -> Result<(), WorkerProtocolError> {
    let _ = write_control(
        output,
        &ControlFrame::Error {
            kind,
            detail: error.to_string(),
        },
    );
    let _ = output.flush();
    Err(error)
}

#[cfg(test)]
pub(crate) fn receive_worker_catalog(
    input: &mut dyn Read,
    sink: &mut dyn CatalogSink,
) -> Result<WorkerCatalog, WorkerProtocolError> {
    receive_worker_hello(input)?;
    receive_worker_catalog_body(input, sink, &[])
}

pub(crate) fn receive_worker_hello(input: &mut dyn Read) -> Result<(), WorkerProtocolError> {
    match read_control(input)? {
        ControlFrame::Hello { version } if version == PROTOCOL_VERSION => Ok(()),
        ControlFrame::Hello { version } => Err(WorkerProtocolError::Invalid(format!(
            "unsupported worker protocol version {version}"
        ))),
        ControlFrame::Error { kind, detail } => Err(reported_error(kind, detail)),
        _ => Err(WorkerProtocolError::Invalid(
            "worker stream did not begin with hello".to_owned(),
        )),
    }
}

#[cfg(test)]
pub(crate) fn receive_worker_catalog_body(
    input: &mut dyn Read,
    sink: &mut dyn CatalogSink,
    replay_candidates: &[ReplayCandidate],
) -> Result<WorkerCatalog, WorkerProtocolError> {
    receive_worker_catalog_body_with_progress(
        input,
        sink,
        replay_candidates,
        &mut None,
        &mut false,
        &mut false,
        &mut || {},
    )
}

pub(crate) fn receive_worker_catalog_body_with_progress(
    input: &mut dyn Read,
    sink: &mut dyn CatalogSink,
    replay_candidates: &[ReplayCandidate],
    active_unit: &mut Option<RecoveryUnit>,
    active_unit_replayed: &mut bool,
    active_unit_committed: &mut bool,
    progress: &mut dyn FnMut(),
) -> Result<WorkerCatalog, WorkerProtocolError> {
    let mut discarding_candidate = false;
    let mut replay_index = 0_usize;
    let mut discarded_message_id = None;
    let mut occurrences = HashMap::new();
    loop {
        let frame = read_control(input)?;
        progress();
        match &frame {
            ControlFrame::UnitStart { unit } => {
                if active_unit.is_some() {
                    return Err(WorkerProtocolError::Invalid(
                        "worker nested recovery units".to_owned(),
                    ));
                }
                send_to_sink(sink, CatalogEvent::UnitStart(*unit))?;
                *active_unit = Some(*unit);
                *active_unit_replayed = false;
                *active_unit_committed = false;
                continue;
            }
            ControlFrame::UnitEnd { unit } => {
                if *active_unit != Some(*unit) {
                    return Err(WorkerProtocolError::Invalid(
                        "worker ended a different recovery unit".to_owned(),
                    ));
                }
                send_to_sink(sink, CatalogEvent::UnitEnd(*unit))?;
                *active_unit = None;
                *active_unit_replayed = false;
                *active_unit_committed = false;
                continue;
            }
            _ => {}
        }
        if discarding_candidate {
            match frame {
                ControlFrame::AttachmentData { byte_len, .. }
                | ControlFrame::PropertyData { byte_len, .. } => {
                    let _ = read_payload(input, byte_len)?;
                    progress();
                }
                ControlFrame::MessageEnd { id, .. } => {
                    if Some(id) != discarded_message_id {
                        return Err(WorkerProtocolError::Invalid(
                            "replayed candidate ended with a different identifier".to_owned(),
                        ));
                    }
                    discarding_candidate = false;
                    discarded_message_id = None;
                }
                ControlFrame::MessageStart { .. } => {
                    return Err(WorkerProtocolError::Invalid(
                        "worker nested a candidate during replay".to_owned(),
                    ));
                }
                ControlFrame::Error { kind, detail } => {
                    return Err(reported_error(kind, detail));
                }
                ControlFrame::Complete { .. } | ControlFrame::Hello { .. } => {
                    return Err(WorkerProtocolError::Invalid(
                        "worker ended while replaying a candidate".to_owned(),
                    ));
                }
                _ => {}
            }
            continue;
        }
        if let ControlFrame::MessageStart {
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
            ..
        } = &frame
            && let Some(expected) = replay_candidates.get(replay_index)
        {
            let occurrence = occurrences
                .entry((*provenance, *id, *recovery_index))
                .or_insert(0_u32);
            let metadata = json!({
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
            });
            if *id != expected.id
                || *provenance != expected.provenance
                || *recovery_index != expected.recovery_index
                || *occurrence != expected.occurrence
                || metadata != expected.metadata
            {
                return Err(WorkerProtocolError::Invalid(
                    "worker replay order does not match the durable ledger".to_owned(),
                ));
            }
            replay_index += 1;
            *active_unit_replayed = true;
            *occurrence = occurrence.saturating_add(1);
            discarding_candidate = true;
            discarded_message_id = Some(*id);
            continue;
        }
        match frame {
            ControlFrame::Hello { .. } => {
                return Err(WorkerProtocolError::Invalid(
                    "duplicate worker hello".to_owned(),
                ));
            }
            ControlFrame::Complete { catalog } => {
                if replay_index != replay_candidates.len() {
                    return Err(WorkerProtocolError::Invalid(
                        "worker completed before replayed candidates were observed".to_owned(),
                    ));
                }
                require_end_of_stream(input)?;
                return Ok(catalog);
            }
            ControlFrame::Error { kind, detail } => return Err(reported_error(kind, detail)),
            ControlFrame::AttachmentData {
                message_id,
                index,
                byte_len,
            } => {
                let bytes = read_payload(input, byte_len)?;
                progress();
                send_to_sink(
                    sink,
                    CatalogEvent::AttachmentData {
                        message_id,
                        index,
                        bytes: &bytes,
                    },
                )?;
            }
            ControlFrame::PropertyData {
                descriptor,
                byte_len,
            } => {
                let bytes = read_payload(input, byte_len)?;
                progress();
                send_to_sink(
                    sink,
                    CatalogEvent::PropertyData {
                        descriptor,
                        bytes: &bytes,
                    },
                )?;
            }
            other => {
                let committed = matches!(other, ControlFrame::MessageEnd { .. });
                send_control_to_sink(sink, other)?;
                if committed {
                    *active_unit_committed = true;
                }
            }
        }
    }
}

fn send_control_to_sink(
    sink: &mut dyn CatalogSink,
    frame: ControlFrame,
) -> Result<(), WorkerProtocolError> {
    let event = match frame {
        ControlFrame::UnitStart { unit } => CatalogEvent::UnitStart(unit),
        ControlFrame::UnitEnd { unit } => CatalogEvent::UnitEnd(unit),
        ControlFrame::Folder {
            id,
            parent_id,
            name,
        } => CatalogEvent::Folder {
            id,
            parent_id,
            name,
        },
        ControlFrame::MessageStart {
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
        } => CatalogEvent::MessageStart {
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
        },
        ControlFrame::Recipient {
            message_id,
            index,
            recipient_type,
            display_name,
            email_address,
            address_type,
        } => CatalogEvent::Recipient {
            message_id,
            index,
            recipient_type,
            display_name,
            email_address,
            address_type,
        },
        ControlFrame::AttachmentStart {
            message_id,
            index,
            attachment_type,
            data_size,
            filename,
        } => CatalogEvent::AttachmentStart {
            message_id,
            index,
            attachment_type,
            data_size,
            filename,
        },
        ControlFrame::AttachmentEnd { message_id, index } => {
            CatalogEvent::AttachmentEnd { message_id, index }
        }
        ControlFrame::AttachmentAbort { message_id, index } => {
            CatalogEvent::AttachmentAbort { message_id, index }
        }
        ControlFrame::PropertyStart { descriptor } => CatalogEvent::PropertyStart(descriptor),
        ControlFrame::PropertyEnd { descriptor } => CatalogEvent::PropertyEnd(descriptor),
        ControlFrame::PropertyAbort { descriptor, reason } => {
            CatalogEvent::PropertyAbort { descriptor, reason }
        }
        ControlFrame::MessageEnd { id, complete } => CatalogEvent::MessageEnd { id, complete },
        ControlFrame::AttachmentData { .. }
        | ControlFrame::PropertyData { .. }
        | ControlFrame::Hello { .. }
        | ControlFrame::Error { .. }
        | ControlFrame::Complete { .. } => {
            return Err(WorkerProtocolError::Invalid(
                "invalid control frame dispatch".to_owned(),
            ));
        }
    };
    send_to_sink(sink, event)
}

fn reported_error(kind: WorkerFailureKind, detail: String) -> WorkerProtocolError {
    match kind {
        WorkerFailureKind::Source => WorkerProtocolError::ReportedSource(detail),
        WorkerFailureKind::Parser => WorkerProtocolError::ReportedParser(detail),
    }
}

fn send_to_sink(
    sink: &mut dyn CatalogSink,
    event: CatalogEvent<'_>,
) -> Result<(), WorkerProtocolError> {
    sink.event(event).map_err(WorkerProtocolError::Sink)
}

fn write_control(output: &mut dyn Write, frame: &ControlFrame) -> Result<(), WorkerProtocolError> {
    let json = serde_json::to_vec(frame)?;
    if json.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(WorkerProtocolError::Invalid(
            "control frame exceeds size limit".to_owned(),
        ));
    }
    let length = u32::try_from(json.len())
        .map_err(|_| WorkerProtocolError::Invalid("control frame exceeds u32".to_owned()))?;
    output.write_all(&length.to_be_bytes())?;
    output.write_all(&json)?;
    Ok(())
}

fn read_control(input: &mut dyn Read) -> Result<ControlFrame, WorkerProtocolError> {
    let mut length = [0_u8; 4];
    input.read_exact(&mut length)?;
    let length = usize::try_from(u32::from_be_bytes(length))
        .map_err(|_| WorkerProtocolError::Invalid("frame length exceeds usize".to_owned()))?;
    if length == 0 || length > MAX_CONTROL_FRAME_BYTES {
        return Err(WorkerProtocolError::Invalid(
            "control frame length is invalid".to_owned(),
        ));
    }
    let mut json = vec![0_u8; length];
    input.read_exact(&mut json)?;
    Ok(serde_json::from_slice(&json)?)
}

fn read_payload(input: &mut dyn Read, byte_len: u32) -> Result<Vec<u8>, WorkerProtocolError> {
    let byte_len = usize::try_from(byte_len)
        .map_err(|_| WorkerProtocolError::Invalid("payload length exceeds usize".to_owned()))?;
    if byte_len > STREAM_CHUNK_BYTES {
        return Err(WorkerProtocolError::Invalid(
            "worker payload exceeds catalog chunk limit".to_owned(),
        ));
    }
    let mut bytes = vec![0_u8; byte_len];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn require_end_of_stream(input: &mut dyn Read) -> Result<(), WorkerProtocolError> {
    let mut trailing = [0_u8; 1];
    match input.read(&mut trailing) {
        Ok(0) => Ok(()),
        Ok(_) => Err(WorkerProtocolError::Invalid(
            "worker sent data after completion".to_owned(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
            require_end_of_stream(input)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use libpff_sys::{CatalogEvent, CatalogIssue, CatalogProvenance, CatalogSink, RawCatalog};
    use pstforge_job::ReplayCandidate;
    use serde_json::json;

    use super::{
        ControlFrame, ProtocolSink, receive_worker_catalog, receive_worker_catalog_body,
        receive_worker_hello, write_control,
    };

    #[derive(Default)]
    struct RecordingSink {
        messages: Vec<u32>,
        payload: Vec<u8>,
    }

    impl CatalogSink for RecordingSink {
        fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
            match event {
                CatalogEvent::MessageStart { id, .. } => self.messages.push(id),
                CatalogEvent::AttachmentData { bytes, .. } => {
                    self.payload.extend_from_slice(bytes);
                }
                _ => {}
            }
            Ok(())
        }
    }

    #[test]
    fn framed_protocol_round_trips_metadata_and_raw_payload() {
        let mut bytes = Vec::new();
        let mut output = ProtocolSink::start(&mut bytes).expect("start protocol");
        output
            .event(CatalogEvent::MessageStart {
                id: 7,
                provenance: CatalogProvenance::Recovered,
                recovery_index: Some(3),
                folder_id: None,
                parent_message_id: None,
                parent_attachment_index: None,
                embedded_path: Vec::new(),
                item_type: Some(11),
                message_class: Some("IPM.Note".to_owned()),
                subject: None,
                sender_name: None,
                sender_email: None,
                submit_filetime: None,
                delivery_filetime: None,
                supported: true,
            })
            .expect("message frame");
        output
            .event(CatalogEvent::AttachmentData {
                message_id: 7,
                index: 0,
                bytes: b"raw\0payload",
            })
            .expect("payload frame");
        output
            .complete(RawCatalog {
                messages: 1,
                recovered_messages: 1,
                ..RawCatalog::default()
            })
            .expect("complete protocol");
        let mut sink = RecordingSink::default();
        let catalog =
            receive_worker_catalog(&mut Cursor::new(bytes), &mut sink).expect("receive protocol");
        assert_eq!(sink.messages, vec![7]);
        assert_eq!(sink.payload, b"raw\0payload");
        assert_eq!(catalog.recovered_messages, 1);
    }

    #[test]
    fn oversized_payload_header_is_rejected_before_allocation() {
        let mut bytes = Vec::new();
        write_control(&mut bytes, &ControlFrame::Hello { version: 1 }).expect("hello");
        write_control(
            &mut bytes,
            &ControlFrame::AttachmentData {
                message_id: 1,
                index: 0,
                byte_len: 65_537,
            },
        )
        .expect("data header");
        let mut sink = RecordingSink::default();
        assert!(receive_worker_catalog(&mut Cursor::new(bytes), &mut sink).is_err());
    }

    #[test]
    fn completion_transmits_issue_counts_without_issue_text() {
        let mut bytes = Vec::new();
        let mut output = ProtocolSink::start(&mut bytes).expect("start protocol");
        output
            .complete(RawCatalog {
                issues: vec![CatalogIssue {
                    node_id: Some(7),
                    operation: "test",
                    message: "private diagnostic".repeat(100_000),
                }],
                issues_dropped: 4,
                ..RawCatalog::default()
            })
            .expect("complete protocol");
        assert!(bytes.len() < 1_024);
        let mut sink = RecordingSink::default();
        let catalog =
            receive_worker_catalog(&mut Cursor::new(bytes), &mut sink).expect("receive completion");
        assert_eq!(catalog.issues, 1);
        assert_eq!(catalog.issues_dropped, 4);
    }

    #[test]
    fn trailing_data_after_completion_is_rejected() {
        let mut bytes = Vec::new();
        ProtocolSink::start(&mut bytes)
            .expect("start protocol")
            .complete(RawCatalog::default())
            .expect("complete protocol");
        bytes.push(0);
        let mut sink = RecordingSink::default();
        assert!(receive_worker_catalog(&mut Cursor::new(bytes), &mut sink).is_err());
    }

    #[test]
    fn replay_skips_committed_candidate_frames_and_payloads() {
        let mut input = Cursor::new({
            let mut bytes = Vec::new();
            let mut output = ProtocolSink::start(&mut bytes).expect("start protocol");
            for id in [1, 2] {
                output
                    .event(CatalogEvent::MessageStart {
                        id,
                        provenance: CatalogProvenance::Normal,
                        recovery_index: None,
                        folder_id: Some(1),
                        parent_message_id: None,
                        parent_attachment_index: None,
                        embedded_path: Vec::new(),
                        item_type: Some(11),
                        message_class: None,
                        subject: None,
                        sender_name: None,
                        sender_email: None,
                        submit_filetime: None,
                        delivery_filetime: None,
                        supported: true,
                    })
                    .expect("message start");
                output
                    .event(CatalogEvent::MessageEnd { id, complete: true })
                    .expect("message end");
            }
            output
                .complete(RawCatalog {
                    messages: 2,
                    ..RawCatalog::default()
                })
                .expect("complete protocol");
            bytes
        });
        receive_worker_hello(&mut input).expect("hello");
        let mut sink = RecordingSink::default();
        receive_worker_catalog_body(
            &mut input,
            &mut sink,
            &[ReplayCandidate {
                id: 1,
                provenance: CatalogProvenance::Normal,
                recovery_index: None,
                occurrence: 0,
                metadata: json!({
                    "folder_id": 1,
                    "parent_message_id": null,
                    "parent_attachment_index": null,
                    "embedded_path": [],
                    "item_type": 11,
                    "message_class": null,
                    "subject": null,
                    "sender_name": null,
                    "sender_email": null,
                    "submit_filetime": null,
                    "delivery_filetime": null,
                    "supported": true,
                }),
                unit: None,
            }],
        )
        .expect("replay protocol");
        assert_eq!(sink.messages, vec![2]);
    }

    #[test]
    fn replay_rejects_colliding_identifier_with_different_metadata() {
        let mut bytes = Vec::new();
        let mut output = ProtocolSink::start(&mut bytes).expect("start protocol");
        output
            .event(CatalogEvent::MessageStart {
                id: 0,
                provenance: CatalogProvenance::Normal,
                recovery_index: None,
                folder_id: Some(1),
                parent_message_id: None,
                parent_attachment_index: None,
                embedded_path: Vec::new(),
                item_type: Some(11),
                message_class: None,
                subject: Some("second".to_owned()),
                sender_name: None,
                sender_email: None,
                submit_filetime: None,
                delivery_filetime: None,
                supported: true,
            })
            .expect("message start");
        output
            .event(CatalogEvent::MessageEnd {
                id: 0,
                complete: true,
            })
            .expect("message end");
        output
            .complete(RawCatalog {
                messages: 1,
                ..RawCatalog::default()
            })
            .expect("complete protocol");
        let mut input = Cursor::new(bytes);
        receive_worker_hello(&mut input).expect("hello");
        let mut sink = RecordingSink::default();
        let expected = ReplayCandidate {
            id: 0,
            provenance: CatalogProvenance::Normal,
            recovery_index: None,
            occurrence: 0,
            metadata: json!({
                "folder_id": 1,
                "parent_message_id": null,
                "parent_attachment_index": null,
                "embedded_path": [],
                "item_type": 11,
                "message_class": null,
                "subject": "first",
                "sender_name": null,
                "sender_email": null,
                "submit_filetime": null,
                "delivery_filetime": null,
                "supported": true,
            }),
            unit: None,
        };
        assert!(receive_worker_catalog_body(&mut input, &mut sink, &[expected]).is_err());
    }
}
