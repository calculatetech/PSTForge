use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::path::Path;

use libpff_sys::{
    CatalogEvent, CatalogProvenance, CatalogSink, NamedPropertyIdentity, PayloadRequest, PffError,
    PffFile, PropertyDescriptor, RawCatalog, RecoveryMode, RecoveryUnit, STREAM_CHUNK_BYTES,
    TraversalOrder,
};
use pstforge_job::ReplayCandidate;
use pstforge_pst::writer::{DirectBlobSource, DirectBlobSpec, WriterError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::{SourceError, SourceFile};

const PROTOCOL_VERSION: u32 = 3;
const MAX_CONTROL_FRAME_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const METADATA_PROPERTY_PREFIX_BYTES: u64 = 64 * 1024;
pub(crate) const METADATA_ATTACHMENT_PREFIX_BYTES: u64 = 16 * 1024;

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
        container_class: Option<String>,
    },
    MessageStart {
        id: u32,
        provenance: CatalogProvenance,
        recovery_index: Option<u64>,
        folder_id: Option<u32>,
        parent_message_id: Option<u32>,
        parent_attachment_index: Option<u32>,
        embedded_path: Vec<u32>,
        associated: bool,
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
    NamedProperty {
        descriptor: PropertyDescriptor,
        identity: NamedPropertyIdentity,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum DirectStreamOwner {
    AttachmentData {
        index: u32,
    },
    Property {
        owner: &'static str,
        owner_index: Option<u32>,
        record_set_index: u32,
        entry_index: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DirectStreamIdentity {
    pub item_key: String,
    pub owner: DirectStreamOwner,
}

pub(crate) struct DirectTopLevelBinding {
    pub item_key: String,
    pub provenance: CatalogProvenance,
    pub source_node_id: Option<u32>,
    pub recovery_index: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DirectMessageBase {
    provenance: CatalogProvenance,
    source_node_id: Option<u32>,
    recovery_index: Option<u64>,
}

pub(crate) struct DirectCandidateBindings {
    embedded: HashMap<(String, u32, DirectMessageBase), String>,
}

impl DirectCandidateBindings {
    pub(crate) fn new() -> Self {
        Self {
            embedded: HashMap::new(),
        }
    }
}

pub(crate) struct DirectProtocolSource<'a> {
    input: &'a mut dyn Read,
    direct_ids: HashMap<DirectStreamIdentity, u64>,
    bindings: DirectCandidateBindings,
    messages: Vec<(u32, String)>,
    attachments: Vec<(u32, u32)>,
    property: Option<PropertyDescriptor>,
    pending: Option<ControlFrame>,
    complete: bool,
    progress: Option<Box<dyn FnMut() + 'a>>,
}

impl<'a> DirectProtocolSource<'a> {
    pub(crate) fn new(
        input: &'a mut dyn Read,
        direct_ids: HashMap<DirectStreamIdentity, u64>,
        bindings: DirectCandidateBindings,
    ) -> Self {
        Self {
            input,
            direct_ids,
            bindings,
            messages: Vec::new(),
            attachments: Vec::new(),
            property: None,
            pending: None,
            complete: false,
            progress: None,
        }
    }

    pub(crate) fn with_progress(mut self, progress: impl FnMut() + 'a) -> Self {
        self.progress = Some(Box::new(progress));
        self
    }

    pub(crate) fn next_top_level_message(
        &mut self,
        expected: Option<DirectTopLevelBinding>,
    ) -> Result<Option<String>, WriterError> {
        if !self.messages.is_empty() {
            return Err(direct_protocol_error(
                "previous direct message was not drained",
            ));
        }
        loop {
            let frame = self.next_control()?;
            match frame {
                ControlFrame::MessageStart {
                    id,
                    provenance,
                    recovery_index,
                    parent_message_id: None,
                    ..
                } => {
                    let expected = expected.ok_or_else(|| {
                        direct_protocol_error(
                            "direct worker emitted an unexpected top-level message",
                        )
                    })?;
                    let source_node_id = (id != 0).then_some(id);
                    if expected.provenance != provenance
                        || expected.source_node_id != source_node_id
                        || expected.recovery_index != recovery_index
                    {
                        return Err(direct_protocol_error(
                            "direct worker top-level order differs from the durable catalog",
                        ));
                    }
                    let key = self.start_message(
                        provenance,
                        id,
                        recovery_index,
                        None,
                        None,
                        Some(expected.item_key),
                    )?;
                    return Ok(Some(key));
                }
                ControlFrame::Complete { .. } => {
                    if expected.is_some() {
                        return Err(direct_protocol_error(
                            "direct worker omitted a durable top-level message",
                        ));
                    }
                    self.complete = true;
                    return Ok(None);
                }
                ControlFrame::Error { kind, detail } => {
                    return Err(direct_protocol_error(format!(
                        "worker reported {kind:?}: {detail}"
                    )));
                }
                other => self.consume_control(other)?,
            }
        }
    }

    pub(crate) fn finish_top_level_message(&mut self) -> Result<(), WriterError> {
        let top_level = self
            .messages
            .first()
            .map(|(_, key)| key.clone())
            .ok_or_else(|| direct_protocol_error("no active direct message"))?;
        while self
            .messages
            .first()
            .is_some_and(|(_, key)| key == &top_level)
        {
            let frame = self.next_control()?;
            self.consume_control(frame)?;
        }
        self.direct_ids.clear();
        self.bindings.embedded.clear();
        Ok(())
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }

    pub(crate) fn require_end_of_stream(&mut self) -> Result<(), WriterError> {
        let mut trailing = [0_u8; 1];
        match self.input.read(&mut trailing) {
            Ok(0) => Ok(()),
            Ok(_) => Err(direct_protocol_error(
                "direct worker emitted trailing bytes after completion",
            )),
            Err(error) => Err(WriterError::Io(direct_worker_payload_io(error.kind()))),
        }
    }

    pub(crate) fn register_stream(
        &mut self,
        identity: DirectStreamIdentity,
        direct_id: u64,
    ) -> Result<(), WriterError> {
        match self.direct_ids.insert(identity, direct_id) {
            None => Ok(()),
            Some(previous) if previous == direct_id => Ok(()),
            Some(_) => Err(direct_protocol_error(
                "direct stream identity maps to multiple catalog identifiers",
            )),
        }
    }

    pub(crate) fn register_embedded_message(
        &mut self,
        parent_item_key: &str,
        parent_attachment_index: u32,
        item_key: &str,
        provenance: CatalogProvenance,
        source_node_id: Option<u32>,
        recovery_index: Option<u64>,
    ) -> Result<(), WriterError> {
        let identity = (
            parent_item_key.to_owned(),
            parent_attachment_index,
            DirectMessageBase {
                provenance,
                source_node_id,
                recovery_index,
            },
        );
        match self.bindings.embedded.insert(identity, item_key.to_owned()) {
            None => Ok(()),
            Some(previous) if previous == item_key => Ok(()),
            Some(_) => Err(direct_protocol_error(
                "direct embedded identity maps to multiple catalog items",
            )),
        }
    }

    fn start_message(
        &mut self,
        provenance: CatalogProvenance,
        id: u32,
        recovery_index: Option<u64>,
        parent_message_id: Option<u32>,
        parent_attachment_index: Option<u32>,
        top_level_item_key: Option<String>,
    ) -> Result<String, WriterError> {
        let source_id = (id != 0).then_some(id);
        let base = DirectMessageBase {
            provenance,
            source_node_id: source_id,
            recovery_index,
        };
        let key = match (parent_message_id, parent_attachment_index) {
            (None, None) => top_level_item_key.ok_or_else(|| {
                direct_protocol_error("direct top-level message has no catalog binding")
            })?,
            (Some(parent_id), Some(index)) => {
                if top_level_item_key.is_some() {
                    return Err(direct_protocol_error(
                        "direct embedded message has a top-level catalog binding",
                    ));
                }
                let (active_parent_id, parent) = self.messages.last().ok_or_else(|| {
                    direct_protocol_error("direct embedded message has no active parent")
                })?;
                if *active_parent_id != parent_id {
                    return Err(direct_protocol_error(
                        "direct embedded message names a non-active parent",
                    ));
                }
                let identity = (parent.clone(), index, base.clone());
                self.bindings
                    .embedded
                    .get(&identity)
                    .cloned()
                    .ok_or_else(|| {
                        let ownership_bindings = self
                            .bindings
                            .embedded
                            .keys()
                            .filter(|(item_key, attachment_index, _)| {
                                item_key == parent && *attachment_index == index
                            })
                            .count();
                        direct_protocol_error(format!(
                            "direct worker message has no embedded catalog identity \
                             (parent={parent}, attachment={index}, \
                             ownership_bindings={ownership_bindings}, \
                             provenance={provenance:?}, source_node_id={source_id:?}, \
                             recovery_index={recovery_index:?})"
                        ))
                    })?
            }
            _ => {
                return Err(direct_protocol_error(
                    "direct worker message has incomplete embedded ownership",
                ));
            }
        };
        self.messages.push((id, key.clone()));
        Ok(key)
    }

    fn next_control(&mut self) -> Result<ControlFrame, WriterError> {
        if let Some(progress) = self.progress.as_mut() {
            progress();
        }
        match self.pending.take() {
            Some(frame) => Ok(frame),
            None => read_control(self.input).map_err(worker_writer_error),
        }
    }

    fn consume_control(&mut self, frame: ControlFrame) -> Result<(), WriterError> {
        match frame {
            ControlFrame::MessageStart {
                id,
                provenance,
                recovery_index,
                parent_message_id,
                parent_attachment_index,
                ..
            } => {
                self.start_message(
                    provenance,
                    id,
                    recovery_index,
                    parent_message_id,
                    parent_attachment_index,
                    None,
                )?;
            }
            ControlFrame::MessageEnd { id, .. } => {
                let (active_id, _) = self
                    .messages
                    .pop()
                    .ok_or_else(|| direct_protocol_error("worker ended an unknown message"))?;
                if active_id != id {
                    return Err(direct_protocol_error(
                        "worker ended a different message than the active message",
                    ));
                }
            }
            ControlFrame::AttachmentStart {
                message_id, index, ..
            } => {
                if self.messages.last().map(|(id, _)| *id) != Some(message_id) {
                    return Err(direct_protocol_error(
                        "worker attachment names a non-active message",
                    ));
                }
                self.attachments.push((message_id, index));
            }
            ControlFrame::AttachmentEnd { message_id, index }
            | ControlFrame::AttachmentAbort { message_id, index } => {
                if self.attachments.pop() != Some((message_id, index)) {
                    return Err(direct_protocol_error(
                        "worker attachment boundaries are inconsistent",
                    ));
                }
            }
            ControlFrame::PropertyStart { descriptor } => {
                self.validate_property_owner(descriptor)?;
                if self.property.replace(descriptor).is_some() {
                    return Err(direct_protocol_error("worker nested property streams"));
                }
            }
            ControlFrame::PropertyEnd { descriptor }
            | ControlFrame::PropertyAbort { descriptor, .. } => {
                if self.property.take() != Some(descriptor) {
                    return Err(direct_protocol_error(
                        "worker property boundaries are inconsistent",
                    ));
                }
            }
            ControlFrame::AttachmentData {
                message_id,
                index,
                byte_len,
            } => {
                if self.attachments.last() != Some(&(message_id, index)) {
                    return Err(direct_protocol_error(
                        "worker attachment payload has inconsistent ownership",
                    ));
                }
                discard_payload(self.input, byte_len, &mut self.progress)
                    .map_err(worker_writer_error)?;
            }
            ControlFrame::PropertyData {
                descriptor,
                byte_len,
            } => {
                self.validate_property_owner(descriptor)?;
                if self.property != Some(descriptor) {
                    return Err(direct_protocol_error(
                        "worker property payload has inconsistent ownership",
                    ));
                }
                discard_payload(self.input, byte_len, &mut self.progress)
                    .map_err(worker_writer_error)?;
            }
            ControlFrame::Complete { .. } => self.complete = true,
            ControlFrame::Error { kind, detail } => {
                return Err(direct_protocol_error(format!(
                    "worker reported {kind:?}: {detail}"
                )));
            }
            ControlFrame::Hello { .. } => {
                return Err(direct_protocol_error("worker repeated its protocol hello"));
            }
            ControlFrame::UnitStart { .. }
            | ControlFrame::UnitEnd { .. }
            | ControlFrame::Folder { .. }
            | ControlFrame::Recipient { .. }
            | ControlFrame::NamedProperty { .. } => {}
        }
        Ok(())
    }

    fn validate_property_owner(&self, descriptor: PropertyDescriptor) -> Result<(), WriterError> {
        let (message_id, attachment_index) = match descriptor.owner {
            libpff_sys::PropertyOwner::Message(message_id)
            | libpff_sys::PropertyOwner::Recipient { message_id, .. } => (message_id, None),
            libpff_sys::PropertyOwner::Attachment { message_id, index } => {
                (message_id, Some(index))
            }
            libpff_sys::PropertyOwner::Folder(_) => {
                return if self.messages.is_empty() && self.attachments.is_empty() {
                    Ok(())
                } else {
                    Err(direct_protocol_error(
                        "direct message stream contains a folder property",
                    ))
                };
            }
        };
        if self.messages.last().map(|(id, _)| *id) != Some(message_id) {
            return Err(direct_protocol_error(
                "worker property names a non-active message",
            ));
        }
        if let Some(index) = attachment_index
            && self.attachments.last() != Some(&(message_id, index))
        {
            return Err(direct_protocol_error(
                "worker property names a non-active attachment",
            ));
        }
        Ok(())
    }

    fn current_data_identity(
        &self,
        frame: &ControlFrame,
    ) -> Result<Option<DirectStreamIdentity>, WriterError> {
        let Some((_, item_key)) = self.messages.last() else {
            return Ok(None);
        };
        let item_key = item_key.clone();
        let owner = match frame {
            ControlFrame::AttachmentData {
                message_id, index, ..
            } => {
                if self.attachments.last() != Some(&(*message_id, *index)) {
                    return Err(direct_protocol_error(
                        "worker attachment payload has inconsistent ownership",
                    ));
                }
                DirectStreamOwner::AttachmentData { index: *index }
            }
            ControlFrame::PropertyData { descriptor, .. } => {
                self.validate_property_owner(*descriptor)?;
                if self.property != Some(*descriptor) {
                    return Err(direct_protocol_error(
                        "worker property payload has inconsistent ownership",
                    ));
                }
                let (owner, owner_index) = match descriptor.owner {
                    libpff_sys::PropertyOwner::Message(_) => ("message", None),
                    libpff_sys::PropertyOwner::Recipient { index, .. } => {
                        ("recipient", Some(index))
                    }
                    libpff_sys::PropertyOwner::Attachment { index, .. } => {
                        ("attachment", Some(index))
                    }
                    libpff_sys::PropertyOwner::Folder(_) => return Ok(None),
                };
                DirectStreamOwner::Property {
                    owner,
                    owner_index,
                    record_set_index: descriptor.record_set_index,
                    entry_index: descriptor.entry_index,
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(DirectStreamIdentity { item_key, owner }))
    }
}

impl DirectBlobSource for DirectProtocolSource<'_> {
    fn open_blob<'a>(
        &'a mut self,
        blob: &DirectBlobSpec,
    ) -> Result<Box<dyn Read + 'a>, WriterError> {
        let top_level = self
            .messages
            .first()
            .map(|(_, key)| key.clone())
            .ok_or_else(|| direct_protocol_error("direct stream requested outside a message"))?;
        loop {
            let frame = self.next_control()?;
            let identity = self.current_data_identity(&frame)?;
            if identity
                .as_ref()
                .and_then(|identity| self.direct_ids.get(identity))
                == Some(&blob.id)
            {
                let remaining = match frame {
                    ControlFrame::AttachmentData { byte_len, .. }
                    | ControlFrame::PropertyData { byte_len, .. } => byte_len,
                    _ => {
                        return Err(direct_protocol_error(
                            "direct identity does not name a payload frame",
                        ));
                    }
                };
                return Ok(Box::new(DirectPayloadReader {
                    source: self,
                    identity: identity.ok_or_else(|| {
                        direct_protocol_error("direct payload identity disappeared")
                    })?,
                    remaining,
                    finished: false,
                }));
            }
            self.consume_control(frame)?;
            if self
                .messages
                .first()
                .is_none_or(|(_, key)| key != &top_level)
            {
                return Err(direct_protocol_error(
                    "requested direct stream was absent from its top-level message",
                ));
            }
        }
    }
}

struct DirectPayloadReader<'a, 'b> {
    source: &'a mut DirectProtocolSource<'b>,
    identity: DirectStreamIdentity,
    remaining: u32,
    finished: bool,
}

#[derive(Debug)]
pub(crate) struct DirectWorkerPayloadIo;

impl std::fmt::Display for DirectWorkerPayloadIo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("direct worker payload stream failed")
    }
}

impl std::error::Error for DirectWorkerPayloadIo {}

fn direct_worker_payload_io(kind: std::io::ErrorKind) -> std::io::Error {
    std::io::Error::new(kind, DirectWorkerPayloadIo)
}

impl Read for DirectPayloadReader<'_, '_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() || self.finished {
            return Ok(0);
        }
        if self.remaining != 0 {
            if let Some(progress) = self.source.progress.as_mut() {
                progress();
            }
            let requested = output
                .len()
                .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
            let read = self
                .source
                .input
                .read(&mut output[..requested])
                .map_err(|error| direct_worker_payload_io(error.kind()))?;
            if read == 0 {
                return Err(direct_worker_payload_io(std::io::ErrorKind::UnexpectedEof));
            }
            self.remaining = self
                .remaining
                .saturating_sub(u32::try_from(read).unwrap_or(u32::MAX));
            return Ok(read);
        }
        let frame = self
            .source
            .next_control()
            .map_err(|_| direct_worker_payload_io(std::io::ErrorKind::InvalidData))?;
        let identity = self
            .source
            .current_data_identity(&frame)
            .map_err(|_| direct_worker_payload_io(std::io::ErrorKind::InvalidData))?;
        if identity.as_ref() == Some(&self.identity) {
            self.remaining = match frame {
                ControlFrame::AttachmentData { byte_len, .. }
                | ControlFrame::PropertyData { byte_len, .. } => byte_len,
                _ => 0,
            };
            return self.read(output);
        }
        self.source.pending = Some(frame);
        self.finished = true;
        Ok(0)
    }
}

fn discard_payload(
    input: &mut dyn Read,
    byte_len: u32,
    progress: &mut Option<Box<dyn FnMut() + '_>>,
) -> Result<(), WorkerProtocolError> {
    let mut remaining = u64::from(byte_len);
    let mut buffer = [0_u8; STREAM_CHUNK_BYTES];
    while remaining != 0 {
        if let Some(progress) = progress.as_mut() {
            progress();
        }
        let requested =
            usize::try_from(remaining.min(u64::try_from(buffer.len()).unwrap_or(u64::MAX)))
                .map_err(|_| {
                    WorkerProtocolError::Invalid("payload discard exceeds usize".to_owned())
                })?;
        input.read_exact(&mut buffer[..requested])?;
        remaining = remaining.saturating_sub(u64::try_from(requested).unwrap_or(u64::MAX));
    }
    Ok(())
}

fn worker_writer_error(error: WorkerProtocolError) -> WriterError {
    WriterError::InvalidStructure(format!("direct worker protocol failed: {error}"))
}

fn direct_protocol_error(detail: impl Into<String>) -> WriterError {
    WriterError::InvalidStructure(format!("direct worker protocol failed: {}", detail.into()))
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
    metadata_only: bool,
    writer_order: bool,
}

impl ProtocolSink<'_> {
    fn start(
        output: &mut dyn Write,
        metadata_only: bool,
        writer_order: bool,
    ) -> Result<ProtocolSink<'_>, WorkerProtocolError> {
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
            metadata_only,
            writer_order,
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
        if let Some(byte_count) = std::env::var("PSTFORGE_INTERNAL_ABORT_MID_PAYLOAD_AFTER_BYTES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|_| bytes.len() >= 1024)
        {
            let prefix_len = byte_count.min(bytes.len());
            self.output
                .write_all(&bytes[..prefix_len])
                .map_err(|error| WorkerProtocolError::Io(error).to_string())?;
            self.output
                .flush()
                .map_err(|error| WorkerProtocolError::Io(error).to_string())?;
            std::process::abort();
        }
        self.output
            .write_all(bytes)
            .map_err(|error| WorkerProtocolError::Io(error).to_string())
    }
}

impl CatalogSink for ProtocolSink<'_> {
    fn property_payload(&self, descriptor: PropertyDescriptor) -> PayloadRequest {
        if self.metadata_only {
            PayloadRequest::Prefix(descriptor.data_size.min(METADATA_PROPERTY_PREFIX_BYTES))
        } else {
            PayloadRequest::Full
        }
    }

    fn attachment_payload(
        &self,
        _message_id: u32,
        _index: u32,
        declared_size: Option<u64>,
    ) -> PayloadRequest {
        if self.metadata_only {
            PayloadRequest::Prefix(
                declared_size
                    .unwrap_or_default()
                    .min(METADATA_ATTACHMENT_PREFIX_BYTES),
            )
        } else {
            PayloadRequest::Full
        }
    }

    fn traversal_order(&self) -> TraversalOrder {
        if self.writer_order {
            TraversalOrder::Writer
        } else {
            TraversalOrder::Source
        }
    }

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
                container_class,
            } => self.send(&ControlFrame::Folder {
                id,
                parent_id,
                name,
                container_class,
            }),
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
            } => self.send(&ControlFrame::MessageStart {
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
            CatalogEvent::NamedProperty {
                descriptor,
                identity,
            } => self.send(&ControlFrame::NamedProperty {
                descriptor,
                identity,
            }),
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
    metadata_only: bool,
    writer_order: bool,
    output: &mut dyn Write,
) -> Result<(), WorkerProtocolError> {
    arm_parent_death_signal()?;
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
    let mut sink = ProtocolSink::start(output, metadata_only, writer_order)?;
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

fn arm_parent_death_signal() -> Result<(), WorkerProtocolError> {
    let expected_parent = std::env::var("PSTFORGE_INTERNAL_SUPERVISOR_PID")
        .map_err(|_| {
            WorkerProtocolError::Invalid("worker supervisor identity is absent".to_owned())
        })?
        .parse::<i32>()
        .map_err(|_| {
            WorkerProtocolError::Invalid("worker supervisor identity is invalid".to_owned())
        })?;
    rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))
        .map_err(|source| WorkerProtocolError::Io(source.into()))?;
    let actual_parent = rustix::process::Pid::as_raw(rustix::process::getppid());
    if actual_parent != expected_parent {
        return Err(WorkerProtocolError::Invalid(
            "worker supervisor exited during startup".to_owned(),
        ));
    }
    Ok(())
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
    sink: &mut dyn ReplayCatalogSink,
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
    sink: &mut dyn ReplayCatalogSink,
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
    sink: &mut dyn ReplayCatalogSink,
    replay_candidates: &[ReplayCandidate],
    active_unit: &mut Option<RecoveryUnit>,
    active_unit_replayed: &mut bool,
    active_unit_committed: &mut bool,
    progress: &mut dyn FnMut(),
) -> Result<WorkerCatalog, WorkerProtocolError> {
    let mut discarding_candidate = false;
    let mut replay_signatures = HashMap::<String, Vec<&ReplayCandidate>>::new();
    for candidate in replay_candidates {
        let signature = replay_signature(
            candidate.provenance,
            candidate.recovery_index,
            candidate.unit,
            &candidate.metadata,
        )?;
        replay_signatures
            .entry(signature)
            .or_default()
            .push(candidate);
    }
    let mut replay_remaining = replay_candidates.len();
    let mut discarded_message_id = None;
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
            associated,
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
        {
            let metadata = json!({
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
            });
            let signature =
                replay_signature(*provenance, *recovery_index, *active_unit, &metadata)?;
            if let Some(candidate) = replay_signatures.get_mut(&signature).and_then(Vec::pop) {
                replay_remaining = replay_remaining.checked_sub(1).ok_or_else(|| {
                    WorkerProtocolError::Invalid(
                        "worker replay candidate count underflow".to_owned(),
                    )
                })?;
                sink.record_replayed_candidate(candidate, *id)?;
                *active_unit_replayed = true;
                discarding_candidate = true;
                discarded_message_id = Some(*id);
                continue;
            }
        }
        match frame {
            ControlFrame::Hello { .. } => {
                return Err(WorkerProtocolError::Invalid(
                    "duplicate worker hello".to_owned(),
                ));
            }
            ControlFrame::Complete { catalog } => {
                if replay_remaining != 0 {
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

pub(crate) trait ReplayCatalogSink: CatalogSink {
    fn record_replayed_candidate(
        &mut self,
        candidate: &ReplayCandidate,
        observed_id: u32,
    ) -> Result<(), WorkerProtocolError>;
}

impl ReplayCatalogSink for pstforge_job::DurableCatalogSink {
    fn record_replayed_candidate(
        &mut self,
        candidate: &ReplayCandidate,
        observed_id: u32,
    ) -> Result<(), WorkerProtocolError> {
        self.record_replayed_candidate(candidate, observed_id)
            .map_err(|error| WorkerProtocolError::Sink(error.to_string()))
    }
}

fn replay_signature(
    provenance: CatalogProvenance,
    recovery_index: Option<u64>,
    unit: Option<RecoveryUnit>,
    metadata: &serde_json::Value,
) -> Result<String, WorkerProtocolError> {
    let mut stable_metadata = metadata.as_object().cloned().ok_or_else(|| {
        WorkerProtocolError::Invalid("worker replay metadata is not an object".to_owned())
    })?;
    stable_metadata.remove("parent_message_id");
    serde_json::to_string(&(provenance, recovery_index, unit, stable_metadata))
        .map_err(WorkerProtocolError::Json)
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
            container_class,
        } => CatalogEvent::Folder {
            id,
            parent_id,
            name,
            container_class,
        },
        ControlFrame::MessageStart {
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
        } => CatalogEvent::MessageStart {
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
        ControlFrame::NamedProperty {
            descriptor,
            identity,
        } => CatalogEvent::NamedProperty {
            descriptor,
            identity,
        },
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
    use std::collections::HashMap;
    use std::io::Cursor;

    use libpff_sys::{
        CatalogEvent, CatalogIssue, CatalogProvenance, CatalogSink, NamedPropertyIdentity,
        NamedPropertyName, PayloadRequest, PropertyDescriptor, PropertyOwner, RawCatalog,
        TraversalOrder,
    };
    use pstforge_job::ReplayCandidate;
    use serde_json::json;

    use super::{
        ControlFrame, DirectCandidateBindings, DirectProtocolSource, DirectStreamIdentity,
        DirectStreamOwner, DirectTopLevelBinding, METADATA_ATTACHMENT_PREFIX_BYTES,
        METADATA_PROPERTY_PREFIX_BYTES, ProtocolSink, ReplayCatalogSink, WorkerCatalog,
        receive_worker_catalog, receive_worker_catalog_body, receive_worker_hello, write_control,
    };
    use pstforge_pst::writer::{DirectBlobSource, DirectBlobSpec};

    #[derive(Default)]
    struct RecordingSink {
        messages: Vec<u32>,
        payload: Vec<u8>,
        named_properties: Vec<NamedPropertyIdentity>,
    }

    impl CatalogSink for RecordingSink {
        fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
            match event {
                CatalogEvent::MessageStart { id, .. } => self.messages.push(id),
                CatalogEvent::AttachmentData { bytes, .. } => {
                    self.payload.extend_from_slice(bytes);
                }
                CatalogEvent::NamedProperty { identity, .. } => {
                    self.named_properties.push(identity);
                }
                _ => {}
            }
            Ok(())
        }
    }

    impl ReplayCatalogSink for RecordingSink {
        fn record_replayed_candidate(
            &mut self,
            _candidate: &ReplayCandidate,
            _observed_id: u32,
        ) -> Result<(), super::WorkerProtocolError> {
            Ok(())
        }
    }

    #[test]
    fn framed_protocol_round_trips_metadata_and_raw_payload() {
        let mut bytes = Vec::new();
        let mut output = ProtocolSink::start(&mut bytes, false, false).expect("start protocol");
        output
            .event(CatalogEvent::MessageStart {
                id: 7,
                provenance: CatalogProvenance::Recovered,
                recovery_index: Some(3),
                folder_id: None,
                parent_message_id: None,
                parent_attachment_index: None,
                embedded_path: Vec::new(),
                associated: false,
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
            .event(CatalogEvent::NamedProperty {
                descriptor: PropertyDescriptor {
                    owner: PropertyOwner::Message(7),
                    record_set_index: 0,
                    entry_index: 0,
                    entry_type: Some(0x8000),
                    value_type: Some(0x0003),
                    data_size: 4,
                },
                identity: NamedPropertyIdentity {
                    guid: [0xAB; 16],
                    name: NamedPropertyName::String("ProtocolCheckpoint".to_owned()),
                },
            })
            .expect("named property frame");
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
        assert_eq!(
            sink.named_properties,
            [NamedPropertyIdentity {
                guid: [0xAB; 16],
                name: NamedPropertyName::String("ProtocolCheckpoint".to_owned()),
            }]
        );
        assert_eq!(catalog.recovered_messages, 1);
    }

    #[test]
    fn metadata_worker_bounds_payloads_and_writer_order_is_explicit() {
        let mut bytes = Vec::new();
        let sink = ProtocolSink::start(&mut bytes, true, true).expect("start protocol");
        let descriptor = PropertyDescriptor {
            owner: libpff_sys::PropertyOwner::Message(7),
            record_set_index: 0,
            entry_index: 0,
            entry_type: Some(0x1000),
            value_type: Some(0x0102),
            data_size: METADATA_PROPERTY_PREFIX_BYTES + 1,
        };
        assert_eq!(
            sink.property_payload(descriptor),
            PayloadRequest::Prefix(METADATA_PROPERTY_PREFIX_BYTES)
        );
        assert_eq!(
            sink.attachment_payload(7, 0, Some(METADATA_ATTACHMENT_PREFIX_BYTES + 1)),
            PayloadRequest::Prefix(METADATA_ATTACHMENT_PREFIX_BYTES)
        );
        assert_eq!(sink.traversal_order(), TraversalOrder::Writer);
    }

    #[test]
    fn direct_protocol_source_matches_requested_streams_and_skips_omissions()
    -> Result<(), Box<dyn std::error::Error>> {
        let property = PropertyDescriptor {
            owner: PropertyOwner::Message(7),
            record_set_index: 0,
            entry_index: 0,
            entry_type: Some(0x1000),
            value_type: Some(0x001f),
            data_size: 4,
        };
        let omitted = PropertyDescriptor {
            entry_index: 1,
            entry_type: Some(0x8000),
            ..property
        };
        let mut bytes = Vec::new();
        write_control(
            &mut bytes,
            &ControlFrame::MessageStart {
                id: 7,
                provenance: CatalogProvenance::Normal,
                recovery_index: None,
                folder_id: Some(1),
                parent_message_id: None,
                parent_attachment_index: None,
                embedded_path: Vec::new(),
                associated: false,
                item_type: Some(11),
                message_class: Some("IPM.Note".to_owned()),
                subject: None,
                sender_name: None,
                sender_email: None,
                submit_filetime: None,
                delivery_filetime: None,
                supported: true,
            },
        )?;
        write_control(
            &mut bytes,
            &ControlFrame::AttachmentStart {
                message_id: 7,
                index: 0,
                attachment_type: Some(i32::from(b'f')),
                data_size: Some(3),
                filename: None,
            },
        )?;
        write_control(
            &mut bytes,
            &ControlFrame::AttachmentData {
                message_id: 7,
                index: 0,
                byte_len: 3,
            },
        )?;
        bytes.extend_from_slice(b"abc");
        write_control(
            &mut bytes,
            &ControlFrame::AttachmentEnd {
                message_id: 7,
                index: 0,
            },
        )?;
        for (descriptor, payload) in [(omitted, b"skip".as_slice()), (property, b"body")] {
            write_control(&mut bytes, &ControlFrame::PropertyStart { descriptor })?;
            write_control(
                &mut bytes,
                &ControlFrame::PropertyData {
                    descriptor,
                    byte_len: u32::try_from(payload.len())?,
                },
            )?;
            bytes.extend_from_slice(payload);
            write_control(&mut bytes, &ControlFrame::PropertyEnd { descriptor })?;
        }
        write_control(
            &mut bytes,
            &ControlFrame::MessageEnd {
                id: 7,
                complete: true,
            },
        )?;
        write_control(
            &mut bytes,
            &ControlFrame::Complete {
                catalog: WorkerCatalog::default(),
            },
        )?;

        let key = "normal:7:-:0".to_owned();
        let direct_ids = HashMap::from([
            (
                DirectStreamIdentity {
                    item_key: key.clone(),
                    owner: DirectStreamOwner::AttachmentData { index: 0 },
                },
                1,
            ),
            (
                DirectStreamIdentity {
                    item_key: key.clone(),
                    owner: DirectStreamOwner::Property {
                        owner: "message",
                        owner_index: None,
                        record_set_index: 0,
                        entry_index: 1,
                    },
                },
                2,
            ),
            (
                DirectStreamIdentity {
                    item_key: key.clone(),
                    owner: DirectStreamOwner::Property {
                        owner: "message",
                        owner_index: None,
                        record_set_index: 0,
                        entry_index: 0,
                    },
                },
                3,
            ),
        ]);
        let missing_bytes = bytes.clone();
        let mut input = Cursor::new(bytes);
        let bindings = DirectCandidateBindings::new();
        let mut source = DirectProtocolSource::new(&mut input, direct_ids, bindings);
        assert_eq!(
            source.next_top_level_message(Some(DirectTopLevelBinding {
                item_key: key.clone(),
                provenance: CatalogProvenance::Normal,
                source_node_id: Some(7),
                recovery_index: None,
            }))?,
            Some(key)
        );
        let mut attachment = Vec::new();
        source
            .open_blob(&DirectBlobSpec {
                id: 1,
                byte_len: 3,
                sha256: None,
            })?
            .read_to_end(&mut attachment)?;
        assert_eq!(attachment, b"abc");
        let mut body = Vec::new();
        source
            .open_blob(&DirectBlobSpec {
                id: 3,
                byte_len: 4,
                sha256: None,
            })?
            .read_to_end(&mut body)?;
        assert_eq!(body, b"body");
        source.finish_top_level_message()?;
        assert_eq!(source.next_top_level_message(None)?, None);
        assert!(source.is_complete());

        let mut missing_input = Cursor::new(missing_bytes);
        let missing_bindings = DirectCandidateBindings::new();
        let mut missing =
            DirectProtocolSource::new(&mut missing_input, HashMap::new(), missing_bindings);
        assert!(
            missing
                .next_top_level_message(Some(DirectTopLevelBinding {
                    item_key: "normal:7:-:0".to_owned(),
                    provenance: CatalogProvenance::Normal,
                    source_node_id: Some(7),
                    recovery_index: None,
                }))?
                .is_some()
        );
        assert!(
            missing
                .open_blob(&DirectBlobSpec {
                    id: 999,
                    byte_len: 1,
                    sha256: None,
                })
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn direct_protocol_uses_parent_ownership_for_duplicate_embedded_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut bytes = Vec::new();
        for (top_id, child_payload) in [(10_u32, b"one".as_slice()), (10, b"two")] {
            write_control(
                &mut bytes,
                &ControlFrame::MessageStart {
                    id: top_id,
                    provenance: CatalogProvenance::Normal,
                    recovery_index: None,
                    folder_id: Some(1),
                    parent_message_id: None,
                    parent_attachment_index: None,
                    embedded_path: Vec::new(),
                    associated: false,
                    item_type: Some(11),
                    message_class: Some("IPM.Note".to_owned()),
                    subject: None,
                    sender_name: None,
                    sender_email: None,
                    submit_filetime: None,
                    delivery_filetime: None,
                    supported: true,
                },
            )?;
            write_control(
                &mut bytes,
                &ControlFrame::AttachmentStart {
                    message_id: top_id,
                    index: 4,
                    attachment_type: Some(5),
                    data_size: None,
                    filename: None,
                },
            )?;
            write_control(
                &mut bytes,
                &ControlFrame::MessageStart {
                    id: 20,
                    provenance: CatalogProvenance::Normal,
                    recovery_index: None,
                    folder_id: None,
                    parent_message_id: Some(top_id),
                    parent_attachment_index: Some(4),
                    embedded_path: vec![4],
                    associated: false,
                    item_type: Some(11),
                    message_class: Some("IPM.Note".to_owned()),
                    subject: None,
                    sender_name: None,
                    sender_email: None,
                    submit_filetime: None,
                    delivery_filetime: None,
                    supported: true,
                },
            )?;
            write_control(
                &mut bytes,
                &ControlFrame::AttachmentStart {
                    message_id: 20,
                    index: 0,
                    attachment_type: Some(i32::from(b'f')),
                    data_size: Some(3),
                    filename: None,
                },
            )?;
            write_control(
                &mut bytes,
                &ControlFrame::AttachmentData {
                    message_id: 20,
                    index: 0,
                    byte_len: 3,
                },
            )?;
            bytes.extend_from_slice(child_payload);
            write_control(
                &mut bytes,
                &ControlFrame::AttachmentEnd {
                    message_id: 20,
                    index: 0,
                },
            )?;
            write_control(
                &mut bytes,
                &ControlFrame::MessageEnd {
                    id: 20,
                    complete: true,
                },
            )?;
            write_control(
                &mut bytes,
                &ControlFrame::AttachmentEnd {
                    message_id: top_id,
                    index: 4,
                },
            )?;
            write_control(
                &mut bytes,
                &ControlFrame::MessageEnd {
                    id: top_id,
                    complete: true,
                },
            )?;
        }
        write_control(
            &mut bytes,
            &ControlFrame::Complete {
                catalog: WorkerCatalog::default(),
            },
        )?;

        let top_a = "normal:10:-:0".to_owned();
        let top_b = "normal:10:-:1".to_owned();
        let child_a = "normal:20:-:0".to_owned();
        let child_b = "normal:20:-:1".to_owned();
        let mut input = Cursor::new(bytes);
        let mut source =
            DirectProtocolSource::new(&mut input, HashMap::new(), DirectCandidateBindings::new());
        for (expected_top, child_key, direct_id, expected_payload) in [
            (top_a, child_a, 1, b"one".as_slice()),
            (top_b, child_b, 2, b"two"),
        ] {
            assert_eq!(
                source.next_top_level_message(Some(DirectTopLevelBinding {
                    item_key: expected_top.clone(),
                    provenance: CatalogProvenance::Normal,
                    source_node_id: Some(10),
                    recovery_index: None,
                }))?,
                Some(expected_top.clone())
            );
            source.register_embedded_message(
                &expected_top,
                4,
                &child_key,
                CatalogProvenance::Normal,
                Some(20),
                None,
            )?;
            source.register_stream(
                DirectStreamIdentity {
                    item_key: child_key,
                    owner: DirectStreamOwner::AttachmentData { index: 0 },
                },
                direct_id,
            )?;
            let mut payload = Vec::new();
            source
                .open_blob(&DirectBlobSpec {
                    id: direct_id,
                    byte_len: 3,
                    sha256: None,
                })?
                .read_to_end(&mut payload)?;
            assert_eq!(payload, expected_payload);
            source.finish_top_level_message()?;
        }
        assert_eq!(source.next_top_level_message(None)?, None);
        source.require_end_of_stream()?;
        Ok(())
    }

    #[test]
    fn oversized_payload_header_is_rejected_before_allocation() {
        let mut bytes = Vec::new();
        write_control(&mut bytes, &ControlFrame::Hello { version: 2 }).expect("hello");
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
        let mut output = ProtocolSink::start(&mut bytes, false, false).expect("start protocol");
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
        ProtocolSink::start(&mut bytes, false, false)
            .expect("start protocol")
            .complete(RawCatalog::default())
            .expect("complete protocol");
        bytes.push(0);
        let mut sink = RecordingSink::default();
        assert!(receive_worker_catalog(&mut Cursor::new(bytes), &mut sink).is_err());
    }

    #[test]
    fn replay_processes_gap_before_committed_candidate_with_shifted_synthetic_id() {
        let mut input = Cursor::new({
            let mut bytes = Vec::new();
            let mut output = ProtocolSink::start(&mut bytes, false, false).expect("start protocol");
            for id in [1, 2] {
                let unit = libpff_sys::RecoveryUnit::Normal {
                    folder: libpff_sys::FolderAddress::root(),
                    folder_id: 1,
                    message_index: u64::from(id - 1),
                };
                output
                    .event(CatalogEvent::UnitStart(unit))
                    .expect("unit start");
                output
                    .event(CatalogEvent::MessageStart {
                        id,
                        provenance: CatalogProvenance::Normal,
                        recovery_index: None,
                        folder_id: Some(1),
                        parent_message_id: None,
                        parent_attachment_index: None,
                        embedded_path: Vec::new(),
                        associated: false,
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
                output.event(CatalogEvent::UnitEnd(unit)).expect("unit end");
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
                item_key: "normal:99:-:0".to_owned(),
                id: 99,
                provenance: CatalogProvenance::Normal,
                recovery_index: None,
                occurrence: 0,
                metadata: json!({
                    "folder_id": 1,
                    "parent_message_id": null,
                    "parent_attachment_index": null,
                    "embedded_path": [],
                    "associated": false,
                    "item_type": 11,
                    "message_class": null,
                    "subject": null,
                    "sender_name": null,
                    "sender_email": null,
                    "submit_filetime": null,
                    "delivery_filetime": null,
                    "supported": true,
                }),
                unit: Some(libpff_sys::RecoveryUnit::Normal {
                    folder: libpff_sys::FolderAddress::root(),
                    folder_id: 1,
                    message_index: 1,
                }),
            }],
        )
        .expect("replay protocol");
        assert_eq!(sink.messages, vec![1]);
    }

    #[test]
    fn replay_rejects_colliding_identifier_with_different_metadata() {
        let mut bytes = Vec::new();
        let mut output = ProtocolSink::start(&mut bytes, false, false).expect("start protocol");
        output
            .event(CatalogEvent::MessageStart {
                id: 0,
                provenance: CatalogProvenance::Normal,
                recovery_index: None,
                folder_id: Some(1),
                parent_message_id: None,
                parent_attachment_index: None,
                embedded_path: Vec::new(),
                associated: false,
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
            item_key: "normal:-:-:0".to_owned(),
            id: 0,
            provenance: CatalogProvenance::Normal,
            recovery_index: None,
            occurrence: 0,
            metadata: json!({
                "folder_id": 1,
                "parent_message_id": null,
                "parent_attachment_index": null,
                "embedded_path": [],
                "associated": false,
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

    #[test]
    fn replay_matches_embedded_candidate_after_parent_identifier_shifts() {
        let unit = libpff_sys::RecoveryUnit::Normal {
            folder: libpff_sys::FolderAddress::root(),
            folder_id: 1,
            message_index: 4,
        };
        let mut bytes = Vec::new();
        let mut output = ProtocolSink::start(&mut bytes, false, false).expect("start protocol");
        output
            .event(CatalogEvent::UnitStart(unit))
            .expect("unit start");
        for (id, parent_message_id, embedded_path) in
            [(20, None, Vec::new()), (21, Some(20), vec![0])]
        {
            output
                .event(CatalogEvent::MessageStart {
                    id,
                    provenance: CatalogProvenance::Normal,
                    recovery_index: None,
                    folder_id: parent_message_id.is_none().then_some(1),
                    parent_message_id,
                    parent_attachment_index: parent_message_id.map(|_| 0),
                    embedded_path,
                    associated: false,
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
        output.event(CatalogEvent::UnitEnd(unit)).expect("unit end");
        output
            .complete(RawCatalog {
                messages: 2,
                ..RawCatalog::default()
            })
            .expect("complete protocol");

        let mut input = Cursor::new(bytes);
        receive_worker_hello(&mut input).expect("hello");
        let mut sink = RecordingSink::default();
        receive_worker_catalog_body(
            &mut input,
            &mut sink,
            &[ReplayCandidate {
                item_key: "normal:99:-:0".to_owned(),
                id: 99,
                provenance: CatalogProvenance::Normal,
                recovery_index: None,
                occurrence: 0,
                metadata: json!({
                    "folder_id": null,
                    "parent_message_id": 98,
                    "parent_attachment_index": 0,
                    "embedded_path": [0],
                    "associated": false,
                    "item_type": 11,
                    "message_class": null,
                    "subject": null,
                    "sender_name": null,
                    "sender_email": null,
                    "submit_filetime": null,
                    "delivery_filetime": null,
                    "supported": true,
                }),
                unit: Some(unit),
            }],
        )
        .expect("embedded replay protocol");
        assert_eq!(sink.messages, vec![20]);
    }

    #[test]
    fn replayed_parent_registers_shifted_identity_for_new_embedded_child()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut sink = pstforge_job::DurableCatalogSink::create(&directory.path().join("job"))?;
        let unit = libpff_sys::RecoveryUnit::Normal {
            folder: libpff_sys::FolderAddress::root(),
            folder_id: 1,
            message_index: 4,
        };
        sink.event(CatalogEvent::UnitStart(unit))?;
        sink.event(CatalogEvent::MessageStart {
            id: 99,
            provenance: CatalogProvenance::Normal,
            recovery_index: None,
            folder_id: None,
            parent_message_id: None,
            parent_attachment_index: None,
            embedded_path: Vec::new(),
            associated: false,
            item_type: Some(11),
            message_class: None,
            subject: None,
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported: true,
        })?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 99,
            index: 0,
            attachment_type: Some(i32::from(b'i')),
            data_size: None,
            filename: None,
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 99,
            index: 0,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 99,
            complete: true,
        })?;
        sink.event(CatalogEvent::UnitEnd(unit))?;
        sink.checkpoint()?;
        let replay = sink.replay_candidates()?;
        assert_eq!(replay.len(), 1);

        let mut bytes = Vec::new();
        let mut output = ProtocolSink::start(&mut bytes, false, false)?;
        output.event(CatalogEvent::UnitStart(unit))?;
        output.event(CatalogEvent::MessageStart {
            id: 20,
            provenance: CatalogProvenance::Normal,
            recovery_index: None,
            folder_id: None,
            parent_message_id: None,
            parent_attachment_index: None,
            embedded_path: Vec::new(),
            associated: false,
            item_type: Some(11),
            message_class: None,
            subject: None,
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported: true,
        })?;
        output.event(CatalogEvent::AttachmentStart {
            message_id: 20,
            index: 0,
            attachment_type: Some(i32::from(b'i')),
            data_size: None,
            filename: None,
        })?;
        output.event(CatalogEvent::AttachmentEnd {
            message_id: 20,
            index: 0,
        })?;
        output.event(CatalogEvent::MessageEnd {
            id: 20,
            complete: true,
        })?;
        output.event(CatalogEvent::MessageStart {
            id: 21,
            provenance: CatalogProvenance::Normal,
            recovery_index: None,
            folder_id: None,
            parent_message_id: Some(20),
            parent_attachment_index: Some(0),
            embedded_path: vec![0],
            associated: false,
            item_type: Some(11),
            message_class: None,
            subject: None,
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported: true,
        })?;
        output.event(CatalogEvent::MessageEnd {
            id: 21,
            complete: true,
        })?;
        output.event(CatalogEvent::UnitEnd(unit))?;
        output.complete(RawCatalog {
            messages: 2,
            embedded_messages: 1,
            ..RawCatalog::default()
        })?;

        let mut input = Cursor::new(bytes);
        receive_worker_hello(&mut input)?;
        receive_worker_catalog_body(&mut input, &mut sink, &replay)?;
        sink.checkpoint()?;
        let ownerships = sink.candidate_ownerships()?;
        let child = ownerships
            .iter()
            .find(|candidate| candidate.source_node_id == Some(21))
            .ok_or("new embedded child was not committed")?;
        assert_eq!(
            child.parent_item_key.as_deref(),
            Some(replay[0].item_key.as_str())
        );
        assert_eq!(child.metadata["parent_message_id"], 99);
        let mail = crate::canonical::load_canonical_mail(&sink)?;
        assert_eq!(mail.len(), 1);
        assert_eq!(mail[0].attachments.len(), 1);
        assert!(mail[0].attachments[0].embedded.is_some());
        Ok(())
    }
}
