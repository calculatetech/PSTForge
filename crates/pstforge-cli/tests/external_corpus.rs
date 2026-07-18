#![deny(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use libpff_sys::{
    CatalogEvent, CatalogSink, NamedPropertyIdentity, NamedPropertyName, PropertyDescriptor,
    PropertyOwner,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const WRITER_MANDATORY_FOLDER_COUNT: u64 = 5;
const NID_IPM_SUBTREE: u32 = 0x8022;
type MatchedSourceMessages = (Vec<Vec<String>>, usize);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RecipientFingerprint {
    index: u32,
    recipient_type: Option<u32>,
    display_name: Option<String>,
    email_address: Option<String>,
    address_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AttachmentFingerprint {
    index: u32,
    attachment_type: Option<i32>,
    filename: Option<String>,
    declared_size: Option<u64>,
    streamed_size: u64,
    sha256: [u8; 32],
    rendering_properties: Vec<PropertyFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PropertyFingerprint {
    id: u32,
    value_type: Option<u32>,
    byte_len: u64,
    sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NamedPropertyFingerprint {
    owner: NamedPropertyOwner,
    identity: NamedPropertyIdentity,
    value_type: Option<u32>,
    byte_len: u64,
    sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NamedPropertyOwner {
    message_class: Option<String>,
    subject: Option<String>,
}

struct ActiveNamedProperty {
    identity: NamedPropertyIdentity,
    value_type: Option<u32>,
    byte_len: u64,
    hasher: Sha256,
}

#[derive(Default)]
struct NamedPropertySink {
    pending: Option<(PropertyDescriptor, NamedPropertyIdentity)>,
    active: BTreeMap<(u32, u32, u32), ActiveNamedProperty>,
    messages: BTreeMap<u32, NamedPropertyOwner>,
    completed: Vec<NamedPropertyFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FolderFingerprint {
    path: Vec<String>,
    deleted_items: bool,
}

#[derive(Default)]
struct IndependentFolderSink {
    folders: BTreeMap<u32, (Option<u32>, Option<String>)>,
    active_classes: BTreeMap<(u32, u32, u32), Vec<u8>>,
    container_classes: BTreeMap<u32, String>,
}

impl CatalogSink for IndependentFolderSink {
    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
        match event {
            CatalogEvent::Folder {
                id,
                parent_id,
                name,
                ..
            } => {
                if self.folders.insert(id, (parent_id, name)).is_some() {
                    return Err("duplicate folder identifier".to_owned());
                }
            }
            CatalogEvent::PropertyStart(descriptor)
                if descriptor.entry_type == Some(0x3613)
                    && matches!(descriptor.owner, PropertyOwner::Folder(_)) =>
            {
                let PropertyOwner::Folder(folder_id) = descriptor.owner else {
                    return Err("folder property owner changed".to_owned());
                };
                self.active_classes.insert(
                    (
                        folder_id,
                        descriptor.record_set_index,
                        descriptor.entry_index,
                    ),
                    Vec::new(),
                );
            }
            CatalogEvent::PropertyData { descriptor, bytes } => {
                let PropertyOwner::Folder(folder_id) = descriptor.owner else {
                    return Ok(());
                };
                if let Some(value) = self.active_classes.get_mut(&(
                    folder_id,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                )) {
                    value.extend_from_slice(bytes);
                }
            }
            CatalogEvent::PropertyEnd(descriptor) => {
                let PropertyOwner::Folder(folder_id) = descriptor.owner else {
                    return Ok(());
                };
                if let Some(bytes) = self.active_classes.remove(&(
                    folder_id,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                )) {
                    let words = bytes
                        .chunks_exact(2)
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .collect::<Vec<_>>();
                    if bytes.len() % 2 != 0 {
                        return Err("folder container class has odd UTF-16 length".to_owned());
                    }
                    let value = String::from_utf16(&words)
                        .map_err(|_| "folder container class is invalid UTF-16")?
                        .trim_end_matches('\0')
                        .to_owned();
                    if self.container_classes.insert(folder_id, value).is_some() {
                        return Err("duplicate folder container class".to_owned());
                    }
                }
            }
            CatalogEvent::PropertyAbort { descriptor, .. } => {
                let PropertyOwner::Folder(folder_id) = descriptor.owner else {
                    return Ok(());
                };
                self.active_classes.remove(&(
                    folder_id,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

impl CatalogSink for NamedPropertySink {
    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
        match event {
            CatalogEvent::MessageStart {
                id,
                message_class,
                subject,
                ..
            } => {
                if self
                    .messages
                    .insert(
                        id,
                        NamedPropertyOwner {
                            message_class,
                            subject,
                        },
                    )
                    .is_some()
                {
                    return Err("duplicate named-property message owner".to_owned());
                }
            }
            CatalogEvent::NamedProperty {
                descriptor,
                identity,
            } => {
                if self.pending.replace((descriptor, identity)).is_some() {
                    return Err("named property identity was not consumed".to_owned());
                }
            }
            CatalogEvent::PropertyStart(descriptor) => {
                let Some((expected, identity)) = self.pending.take() else {
                    return Ok(());
                };
                if descriptor != expected {
                    return Err("named property identity did not precede its value".to_owned());
                }
                let PropertyOwner::Message(message_id) = descriptor.owner else {
                    return Ok(());
                };
                let key = (
                    message_id,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                );
                if self
                    .active
                    .insert(
                        key,
                        ActiveNamedProperty {
                            identity,
                            value_type: descriptor.value_type,
                            byte_len: 0,
                            hasher: Sha256::new(),
                        },
                    )
                    .is_some()
                {
                    return Err("duplicate active named property".to_owned());
                }
            }
            CatalogEvent::PropertyData { descriptor, bytes } => {
                let PropertyOwner::Message(message_id) = descriptor.owner else {
                    return Ok(());
                };
                if let Some(property) = self.active.get_mut(&(
                    message_id,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                )) {
                    property.byte_len = property
                        .byte_len
                        .checked_add(u64::try_from(bytes.len()).map_err(|error| error.to_string())?)
                        .ok_or_else(|| "named property size overflow".to_owned())?;
                    property.hasher.update(bytes);
                }
            }
            CatalogEvent::PropertyEnd(descriptor) => {
                let PropertyOwner::Message(message_id) = descriptor.owner else {
                    return Ok(());
                };
                if let Some(property) = self.active.remove(&(
                    message_id,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                )) {
                    let owner = self
                        .messages
                        .get(&message_id)
                        .cloned()
                        .ok_or_else(|| "named property has no message owner".to_owned())?;
                    self.completed.push(NamedPropertyFingerprint {
                        owner,
                        identity: property.identity,
                        value_type: property.value_type,
                        byte_len: property.byte_len,
                        sha256: property.hasher.finalize().into(),
                    });
                }
            }
            CatalogEvent::PropertyAbort { descriptor, .. } => {
                let PropertyOwner::Message(message_id) = descriptor.owner else {
                    return Ok(());
                };
                self.active.remove(&(
                    message_id,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                ));
            }
            CatalogEvent::MessageEnd { id, .. } => {
                if self.messages.remove(&id).is_none() {
                    return Err("named-property message ended without an owner".to_owned());
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MessageContentFingerprint {
    embedded_path: Vec<u32>,
    message_class: Option<String>,
    subject: Option<String>,
    sender_name: Option<String>,
    sender_email: Option<String>,
    submit_filetime: Option<u64>,
    delivery_filetime: Option<u64>,
    recipients: Vec<RecipientFingerprint>,
    attachments: Vec<AttachmentFingerprint>,
    body_properties: Vec<PropertyFingerprint>,
}

#[derive(Debug, Clone)]
struct MessageFingerprint {
    folder_path: Vec<String>,
    content: MessageContentFingerprint,
    complete: bool,
}

struct ActiveAttachmentFingerprint {
    attachment_type: Option<i32>,
    filename: Option<String>,
    declared_size: Option<u64>,
    streamed_size: u64,
    hasher: Sha256,
    rendering_properties: Vec<PropertyFingerprint>,
}

struct ActivePropertyFingerprint {
    id: u32,
    value_type: Option<u32>,
    byte_len: u64,
    hasher: Sha256,
}

#[derive(Default)]
struct IndependentMessageSink {
    folder_paths: BTreeMap<u32, Vec<String>>,
    active: BTreeMap<u32, MessageFingerprint>,
    attachments: BTreeMap<(u32, u32), ActiveAttachmentFingerprint>,
    properties: BTreeMap<(u32, u32, u32), ActivePropertyFingerprint>,
    attachment_properties: BTreeMap<(u32, u32, u32, u32), ActivePropertyFingerprint>,
    completed: Vec<MessageFingerprint>,
}

impl CatalogSink for IndependentMessageSink {
    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
        match event {
            CatalogEvent::Folder {
                id,
                parent_id,
                name,
                ..
            } => {
                let mut path = match parent_id {
                    Some(parent) => self
                        .folder_paths
                        .get(&parent)
                        .cloned()
                        .ok_or_else(|| "folder preceded its parent".to_owned())?,
                    None => Vec::new(),
                };
                if parent_id.is_some()
                    && id != NID_IPM_SUBTREE
                    && let Some(name) = name
                {
                    path.push(name);
                }
                if self.folder_paths.insert(id, path).is_some() {
                    return Err("duplicate folder identifier".to_owned());
                }
            }
            CatalogEvent::MessageStart {
                id,
                folder_id,
                parent_message_id,
                embedded_path,
                message_class,
                subject,
                sender_name,
                sender_email,
                submit_filetime,
                delivery_filetime,
                supported,
                ..
            } => {
                if !supported {
                    return Ok(());
                }
                let folder_path = match (folder_id, parent_message_id) {
                    (Some(folder), _) => self
                        .folder_paths
                        .get(&folder)
                        .cloned()
                        .ok_or_else(|| "message referenced an unknown folder".to_owned())?,
                    (None, Some(parent)) => self
                        .active
                        .get(&parent)
                        .map(|message| message.folder_path.clone())
                        .ok_or_else(|| {
                            "embedded message referenced an unknown parent".to_owned()
                        })?,
                    (None, None) => Vec::new(),
                };
                let message = MessageFingerprint {
                    folder_path,
                    content: MessageContentFingerprint {
                        embedded_path,
                        message_class,
                        subject,
                        sender_name,
                        sender_email,
                        submit_filetime,
                        delivery_filetime,
                        recipients: Vec::new(),
                        attachments: Vec::new(),
                        body_properties: Vec::new(),
                    },
                    complete: true,
                };
                if self.active.insert(id, message).is_some() {
                    return Err("duplicate active message identifier".to_owned());
                }
            }
            CatalogEvent::Recipient {
                message_id,
                index,
                recipient_type,
                display_name,
                email_address,
                address_type,
            } => {
                if let Some(message) = self.active.get_mut(&message_id) {
                    message.content.recipients.push(RecipientFingerprint {
                        index,
                        recipient_type,
                        display_name,
                        email_address,
                        address_type,
                    });
                }
            }
            CatalogEvent::AttachmentStart {
                message_id,
                index,
                attachment_type,
                data_size,
                filename,
            } if self.active.contains_key(&message_id) => {
                if self
                    .attachments
                    .insert(
                        (message_id, index),
                        ActiveAttachmentFingerprint {
                            attachment_type,
                            filename,
                            declared_size: data_size,
                            streamed_size: 0,
                            hasher: Sha256::new(),
                            rendering_properties: Vec::new(),
                        },
                    )
                    .is_some()
                {
                    return Err("duplicate active attachment".to_owned());
                }
            }
            CatalogEvent::AttachmentData {
                message_id,
                index,
                bytes,
            } => {
                if let Some(attachment) = self.attachments.get_mut(&(message_id, index)) {
                    attachment.streamed_size = attachment
                        .streamed_size
                        .checked_add(u64::try_from(bytes.len()).map_err(|error| error.to_string())?)
                        .ok_or_else(|| "attachment size overflow".to_owned())?;
                    attachment.hasher.update(bytes);
                }
            }
            CatalogEvent::AttachmentEnd { message_id, index } => {
                if let Some(mut attachment) = self.attachments.remove(&(message_id, index)) {
                    let message = self
                        .active
                        .get_mut(&message_id)
                        .ok_or_else(|| "attachment ended without its message".to_owned())?;
                    attachment.rendering_properties.sort();
                    message.content.attachments.push(AttachmentFingerprint {
                        index,
                        attachment_type: attachment.attachment_type,
                        filename: attachment.filename,
                        declared_size: attachment.declared_size,
                        streamed_size: attachment.streamed_size,
                        sha256: attachment.hasher.finalize().into(),
                        rendering_properties: attachment.rendering_properties,
                    });
                }
            }
            CatalogEvent::PropertyStart(descriptor)
                if matches!(descriptor.owner, PropertyOwner::Message(_))
                    && descriptor.entry_type.is_some_and(|id| {
                        matches!(
                            id,
                            0x007d
                                | 0x0e07
                                | 0x1000
                                | 0x1009
                                | 0x1013
                                | 0x3001
                                | 0x3007
                                | 0x3008
                                | 0x3a06
                                | 0x3a08
                                | 0x3a11
                                | 0x3a16
                                | 0x3a17
                                | 0x3a1c
                                | 0x3a42
                                | 0x3fde
                        )
                    }) =>
            {
                let PropertyOwner::Message(message_id) = descriptor.owner else {
                    return Err("message property owner changed".to_owned());
                };
                if self.active.contains_key(&message_id) {
                    let id = descriptor
                        .entry_type
                        .ok_or_else(|| "body property identifier disappeared".to_owned())?;
                    self.properties.insert(
                        (
                            message_id,
                            descriptor.record_set_index,
                            descriptor.entry_index,
                        ),
                        ActivePropertyFingerprint {
                            id,
                            value_type: descriptor.value_type,
                            byte_len: 0,
                            hasher: Sha256::new(),
                        },
                    );
                }
            }
            CatalogEvent::PropertyStart(descriptor)
                if matches!(descriptor.owner, PropertyOwner::Attachment { .. })
                    && descriptor.entry_type.is_some_and(|id| {
                        matches!(id, 0x370b | 0x370e | 0x3712 | 0x3713 | 0x3714)
                    }) =>
            {
                let PropertyOwner::Attachment { message_id, index } = descriptor.owner else {
                    return Err("attachment property owner changed".to_owned());
                };
                if self.attachments.contains_key(&(message_id, index)) {
                    let id = descriptor
                        .entry_type
                        .ok_or_else(|| "attachment property identifier disappeared".to_owned())?;
                    self.attachment_properties.insert(
                        (
                            message_id,
                            index,
                            descriptor.record_set_index,
                            descriptor.entry_index,
                        ),
                        ActivePropertyFingerprint {
                            id,
                            value_type: descriptor.value_type,
                            byte_len: 0,
                            hasher: Sha256::new(),
                        },
                    );
                }
            }
            CatalogEvent::PropertyData { descriptor, bytes } => {
                let property = match descriptor.owner {
                    PropertyOwner::Message(message_id) => self.properties.get_mut(&(
                        message_id,
                        descriptor.record_set_index,
                        descriptor.entry_index,
                    )),
                    PropertyOwner::Attachment { message_id, index } => {
                        self.attachment_properties.get_mut(&(
                            message_id,
                            index,
                            descriptor.record_set_index,
                            descriptor.entry_index,
                        ))
                    }
                    _ => None,
                };
                if let Some(property) = property {
                    property.byte_len = property
                        .byte_len
                        .checked_add(u64::try_from(bytes.len()).map_err(|error| error.to_string())?)
                        .ok_or_else(|| "observed property size overflow".to_owned())?;
                    property.hasher.update(bytes);
                }
            }
            CatalogEvent::PropertyEnd(descriptor) => {
                match descriptor.owner {
                    PropertyOwner::Message(message_id) => {
                        if let Some(property) = self.properties.remove(&(
                            message_id,
                            descriptor.record_set_index,
                            descriptor.entry_index,
                        )) {
                            let message = self.active.get_mut(&message_id).ok_or_else(|| {
                                "observed property ended without its message".to_owned()
                            })?;
                            message
                                .content
                                .body_properties
                                .push(finish_property(property));
                        }
                    }
                    PropertyOwner::Attachment { message_id, index } => {
                        if let Some(property) = self.attachment_properties.remove(&(
                            message_id,
                            index,
                            descriptor.record_set_index,
                            descriptor.entry_index,
                        )) {
                            let attachment =
                                self.attachments.get_mut(&(message_id, index)).ok_or_else(
                                    || "observed property ended without its attachment".to_owned(),
                                )?;
                            attachment
                                .rendering_properties
                                .push(finish_property(property));
                        }
                    }
                    _ => {}
                }
            }
            CatalogEvent::PropertyAbort { descriptor, .. } => match descriptor.owner {
                PropertyOwner::Message(message_id) => {
                    self.properties.remove(&(
                        message_id,
                        descriptor.record_set_index,
                        descriptor.entry_index,
                    ));
                }
                PropertyOwner::Attachment { message_id, index } => {
                    self.attachment_properties.remove(&(
                        message_id,
                        index,
                        descriptor.record_set_index,
                        descriptor.entry_index,
                    ));
                }
                _ => {}
            },
            CatalogEvent::MessageEnd { id, complete } => {
                let Some(mut message) = self.active.remove(&id) else {
                    return Ok(());
                };
                message.complete = complete;
                message.content.recipients.sort();
                message.content.attachments.sort();
                message.content.body_properties.sort();
                self.completed.push(message);
            }
            _ => {}
        }
        Ok(())
    }
}

fn finish_property(property: ActivePropertyFingerprint) -> PropertyFingerprint {
    PropertyFingerprint {
        id: property.id,
        value_type: property.value_type,
        byte_len: property.byte_len,
        sha256: property.hasher.finalize().into(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    path: PathBuf,
    sha256: String,
    classification: String,
    milestone_0_1: bool,
    #[serde(default)]
    milestone_0_1_1: bool,
    minimum_folders: u64,
    minimum_messages: u64,
    #[serde(default)]
    minimum_recipients: u64,
    #[serde(default)]
    minimum_attachments: u64,
    #[serde(default)]
    minimum_raw_properties: u64,
    #[serde(default = "default_peak_chunk_limit")]
    maximum_peak_stream_chunk_bytes: u64,
    #[serde(default)]
    milestone_0_3: bool,
    #[serde(default)]
    minimum_recovered_items: u64,
    #[serde(default)]
    minimum_orphan_items: u64,
    #[serde(default)]
    milestone_0_4: bool,
    #[serde(default = "default_split_limit")]
    milestone_0_4_max_pst_bytes: u64,
    #[serde(default)]
    milestone_0_4_allow_oversize: bool,
}

fn default_split_limit() -> u64 {
    2 * 1024 * 1024
}

fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn independent_messages(
    path: &std::path::Path,
) -> Result<Vec<MessageFingerprint>, Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let native = libpff_sys::PffFile::open_fd(file.as_fd())?;
    let mut sink = IndependentMessageSink::default();
    let catalog = native.catalog(&mut sink)?;
    if catalog.issues.iter().any(|issue| {
        issue.operation != "count attachments"
            || !issue
                .message
                .contains("libpff_message_get_number_of_attachments")
    }) || catalog.issues_dropped != 0
        || !sink.active.is_empty()
        || !sink.attachments.is_empty()
        || !sink.properties.is_empty()
        || !sink.attachment_properties.is_empty()
    {
        return Err("independent message catalog was incomplete".into());
    }
    Ok(sink.completed)
}

fn independent_named_properties(
    path: &std::path::Path,
) -> Result<Vec<NamedPropertyFingerprint>, Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let native = libpff_sys::PffFile::open_fd(file.as_fd())?;
    let mut sink = NamedPropertySink::default();
    let catalog = native.catalog(&mut sink)?;
    if !catalog.issues.is_empty() || catalog.issues_dropped != 0 {
        return Err("named property catalog reported read issues".into());
    }
    if sink.pending.is_some() || !sink.active.is_empty() || !sink.messages.is_empty() {
        return Err("named property stream ended with unfinished state".into());
    }
    Ok(sink.completed)
}

fn independent_visible_folders(
    path: &std::path::Path,
) -> Result<Vec<FolderFingerprint>, Box<dyn std::error::Error>> {
    const NID_IPM_SUBTREE: u32 = 0x8022;
    const NID_DELETED_ITEMS: u32 = 0x8062;

    let file = fs::File::open(path)?;
    let native = libpff_sys::PffFile::open_fd(file.as_fd())?;
    let mut sink = IndependentFolderSink::default();
    let catalog = native.catalog(&mut sink)?;
    if !catalog.issues.is_empty() || catalog.issues_dropped != 0 {
        return Err("folder catalog reported read issues".into());
    }
    let mut output = Vec::new();
    for id in sink
        .folders
        .keys()
        .copied()
        .filter(|id| *id != NID_IPM_SUBTREE)
    {
        let mut current = Some(id);
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut belongs_to_ipm = false;
        while let Some(folder_id) = current {
            if folder_id == NID_IPM_SUBTREE {
                belongs_to_ipm = true;
                break;
            }
            if !seen.insert(folder_id) || seen.len() > 64 {
                return Err("folder hierarchy is cyclic or too deep".into());
            }
            let Some((parent_id, name)) = sink.folders.get(&folder_id) else {
                return Err("folder hierarchy references a missing parent".into());
            };
            chain.push(name.clone());
            current = *parent_id;
        }
        if belongs_to_ipm {
            chain.reverse();
            let path = chain
                .into_iter()
                .map(|name| {
                    name.filter(|value| !value.is_empty())
                        .ok_or("visible folder name is absent")
                })
                .collect::<Result<Vec<_>, _>>()?;
            output.push(FolderFingerprint {
                path,
                deleted_items: id == NID_DELETED_ITEMS,
            });
        }
    }
    output.sort();
    Ok(output)
}

fn independent_folder_classes(
    path: &std::path::Path,
) -> Result<BTreeMap<Vec<String>, String>, Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let native = libpff_sys::PffFile::open_fd(file.as_fd())?;
    let mut sink = IndependentFolderSink::default();
    let catalog = native.catalog(&mut sink)?;
    if !catalog.issues.is_empty() || catalog.issues_dropped != 0 || !sink.active_classes.is_empty()
    {
        return Err("folder-class catalog was incomplete".into());
    }
    let mut output = BTreeMap::new();
    for (id, container_class) in &sink.container_classes {
        let mut current = Some(*id);
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut belongs_to_ipm = false;
        while let Some(folder_id) = current {
            if folder_id == NID_IPM_SUBTREE {
                belongs_to_ipm = true;
                break;
            }
            if !seen.insert(folder_id) || seen.len() > 64 {
                return Err("folder-class hierarchy is cyclic or too deep".into());
            }
            let Some((parent_id, name)) = sink.folders.get(&folder_id) else {
                return Err("folder-class hierarchy references a missing parent".into());
            };
            chain.push(name.clone().filter(|value| !value.is_empty()));
            current = *parent_id;
        }
        if belongs_to_ipm {
            chain.reverse();
            let chain = chain
                .into_iter()
                .map(|name| name.ok_or("visible folder name is absent"))
                .collect::<Result<Vec<_>, _>>()?;
            if output.insert(chain, container_class.clone()).is_some() {
                return Err("duplicate visible folder-class path".into());
            }
        }
    }
    Ok(output)
}

fn verify_exact_message_fidelity(
    expected: Vec<MessageFingerprint>,
    actual: Vec<MessageFingerprint>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (_, remaining) = match_source_messages(&expected, &actual)?;
    if remaining != 0 {
        return Err("generated message multiplicity differs from the source catalog".into());
    }
    Ok(())
}

fn replicated_source_folder_counts(
    expected: &[MessageFingerprint],
    actual: &[MessageFingerprint],
) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let (source_paths, _) = match_source_messages(expected, actual)?;
    let leaf_folders = source_paths.iter().cloned().collect::<BTreeSet<_>>();
    let mut all_folders = BTreeSet::new();
    for source_path in &source_paths {
        for depth in 1..=source_path.len() {
            all_folders.insert(source_path[..depth].to_vec());
        }
    }
    Ok((
        u64::try_from(leaf_folders.len())?,
        u64::try_from(all_folders.len())?,
    ))
}

fn match_source_messages(
    expected: &[MessageFingerprint],
    actual: &[MessageFingerprint],
) -> Result<MatchedSourceMessages, Box<dyn std::error::Error>> {
    let mut unmatched = expected.iter().collect::<Vec<_>>();
    let mut source_paths = Vec::with_capacity(actual.len());
    for generated in actual {
        if !unmatched
            .iter()
            .any(|source| source.content == generated.content)
        {
            let categories = unmatched
                .iter()
                .find(|source| generated.folder_path == source.folder_path)
                .map(|source| {
                    let mut difference =
                        fingerprint_difference(&source.content, &generated.content);
                    if source.content.body_properties != generated.content.body_properties {
                        difference.push("body property IDs logged separately");
                    }
                    difference
                })
                .unwrap_or_else(|| vec!["folder candidate"]);
            let body_ids = unmatched
                .iter()
                .find(|source| generated.folder_path == source.folder_path)
                .map(|source| {
                    (
                        source
                            .content
                            .body_properties
                            .iter()
                            .map(|property| property.id)
                            .collect::<Vec<_>>(),
                        generated
                            .content
                            .body_properties
                            .iter()
                            .map(|property| property.id)
                            .collect::<Vec<_>>(),
                    )
                });
            return Err(format!(
                "generated message content differs from the source catalog in: {}; body IDs: {body_ids:?}",
                categories.join(", "),
            )
            .into());
        }
        let Some(position) = unmatched.iter().position(|source| {
            source.content == generated.content && generated.folder_path == source.folder_path
        }) else {
            let source_depth = unmatched
                .iter()
                .find(|source| source.content == generated.content)
                .map(|source| source.folder_path.len())
                .unwrap_or_default();
            return Err(format!(
                "generated message source folder hierarchy differs (source depth {source_depth}, generated depth {}, generated prefix {:?})",
                generated.folder_path.len(),
                generated.folder_path.get(..2)
            )
            .into());
        };
        source_paths.push(unmatched.swap_remove(position).folder_path.clone());
    }
    Ok((source_paths, unmatched.len()))
}

fn fingerprint_difference(
    source: &MessageContentFingerprint,
    generated: &MessageContentFingerprint,
) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if source.embedded_path != generated.embedded_path {
        fields.push("embedded ownership");
    }
    if source.message_class != generated.message_class {
        fields.push("message class");
    }
    if source.subject != generated.subject {
        fields.push("subject");
    }
    if source.sender_name != generated.sender_name || source.sender_email != generated.sender_email
    {
        fields.push("sender");
    }
    if source.submit_filetime != generated.submit_filetime
        || source.delivery_filetime != generated.delivery_filetime
    {
        fields.push("delivery timestamps");
    }
    if source.recipients != generated.recipients {
        fields.push("recipients");
    }
    if source.attachments != generated.attachments {
        fields.push("attachments");
    }
    if source.body_properties != generated.body_properties {
        fields.push("body properties");
    }
    fields
}

#[test]
#[ignore = "requires the external v042-named-property-source corpus case"]
fn milestone_0_4_2_named_properties_roundtrip_through_libpff()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-named-property-source")
        .ok_or("manifest has no v042-named-property-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256 != case.sha256 {
        return Err("named-property source SHA-256 does not match its manifest".into());
    }

    let source = independent_named_properties(&case.path)?;
    let expected = [
        NamedPropertyFingerprint {
            owner: NamedPropertyOwner {
                message_class: Some("IPM.Note".to_owned()),
                subject: Some("Named property fidelity checkpoint".to_owned()),
            },
            identity: NamedPropertyIdentity {
                guid: [0; 16],
                name: NamedPropertyName::Numeric(0x8005),
            },
            value_type: Some(0x001f),
            byte_len: 50,
            sha256: Sha256::digest(
                "named property checkpoint"
                    .encode_utf16()
                    .flat_map(|unit| unit.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
            .into(),
        },
        NamedPropertyFingerprint {
            owner: NamedPropertyOwner {
                message_class: Some("IPM.Note".to_owned()),
                subject: Some("Named property fidelity checkpoint".to_owned()),
            },
            identity: NamedPropertyIdentity {
                guid: *b"PSTForgeNamedSet",
                name: NamedPropertyName::String("CustomCheckpoint".to_owned()),
            },
            value_type: Some(0x0003),
            byte_len: 4,
            sha256: Sha256::digest(21_i32.to_le_bytes()).into(),
        },
    ];
    if source != expected {
        return Err("named-property source does not match the qualification contract".into());
    }

    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "named-property split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if report["partial"].as_bool() != Some(false)
        || report["parts"].as_array().map(Vec::len) != Some(1)
    {
        return Err("named-property split was not a complete one-part result".into());
    }
    let generated = independent_named_properties(&job.join("parts/part-0001.pst"))?;
    if generated != source {
        return Err("named-property identity or value changed during the split".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the external v042-empty-folders-source corpus case"]
fn milestone_0_4_2_empty_folders_roundtrip_through_libpff() -> Result<(), Box<dyn std::error::Error>>
{
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-empty-folders-source")
        .ok_or("manifest has no v042-empty-folders-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256 != case.sha256 {
        return Err("empty-folder source SHA-256 does not match its manifest".into());
    }

    let source = independent_visible_folders(&case.path)?;
    let mut expected = vec![
        FolderFingerprint {
            path: vec!["Deleted Items".to_owned()],
            deleted_items: true,
        },
        FolderFingerprint {
            path: vec!["Deleted items".to_owned()],
            deleted_items: false,
        },
        FolderFingerprint {
            path: vec!["Empty Parent".to_owned()],
            deleted_items: false,
        },
        FolderFingerprint {
            path: vec!["Empty Parent".to_owned(), "Empty Child".to_owned()],
            deleted_items: false,
        },
        FolderFingerprint {
            path: vec!["Inbox".to_owned()],
            deleted_items: false,
        },
    ];
    expected.sort();
    if source != expected {
        return Err("empty-folder source does not match the qualification contract".into());
    }

    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "empty-folder split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if report["partial"].as_bool() != Some(false)
        || report["parts"].as_array().map(Vec::len) != Some(1)
    {
        return Err("empty-folder split was not a complete one-part result".into());
    }
    let generated = independent_visible_folders(&job.join("parts/part-0001.pst"))?;
    if generated != source {
        return Err("visible empty-folder path or role changed during the split".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the external v042-contact-source corpus case"]
fn milestone_0_4_2_contacts_roundtrip_through_libpff() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-contact-source")
        .ok_or("manifest has no v042-contact-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256 != case.sha256 {
        return Err("contact source SHA-256 does not match its manifest".into());
    }

    let source_messages = independent_messages(&case.path)?;
    if source_messages.len() != 1
        || source_messages[0].folder_path != ["Contacts"]
        || source_messages[0].content.message_class.as_deref() != Some("IPM.Contact")
        || source_messages[0].content.subject.as_deref() != Some("Ada Lovelace")
        || source_messages[0].content.sender_name.is_some()
        || source_messages[0].content.sender_email.is_some()
        || !source_messages[0].complete
    {
        return Err("contact source does not match the item contract".into());
    }
    let expected_property_ids = [
        0x0E07, 0x1000, 0x3001, 0x3007, 0x3008, 0x3A06, 0x3A08, 0x3A11, 0x3A16, 0x3A17, 0x3A1C,
        0x3A42, 0x3FDE,
    ];
    if source_messages[0]
        .content
        .body_properties
        .iter()
        .map(|property| property.id)
        .collect::<Vec<_>>()
        != expected_property_ids
    {
        return Err("contact source ordinary-property set changed".into());
    }
    let source_named = independent_named_properties(&case.path)?;
    let expected_named_ids = [0x8005, 0x8080, 0x8082, 0x8083];
    if source_named.len() != expected_named_ids.len()
        || !source_named
            .iter()
            .zip(expected_named_ids)
            .all(|(property, expected)| {
                property.identity.name == NamedPropertyName::Numeric(expected)
                    && property.identity.guid
                        == [
                            0x04, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00,
                            0x00, 0x00, 0x00, 0x46,
                        ]
            })
    {
        return Err("contact source named-property identity changed".into());
    }
    let source_classes = independent_folder_classes(&case.path)?;
    if source_classes
        .get(&vec!["Contacts".to_owned()])
        .map(String::as_str)
        != Some("IPF.Contact")
    {
        return Err("contact source folder is not IPF.Contact".into());
    }

    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "contact split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if report["partial"].as_bool() != Some(false)
        || report["written_candidates"].as_u64() != Some(1)
        || report["parts"].as_array().map(Vec::len) != Some(1)
        || report["parts"][0]["omitted_folders"].as_u64() != Some(0)
        || report["parts"][0]["omitted_properties"].as_u64() != Some(0)
        || report["parts"][0]["omitted_attachments"].as_u64() != Some(0)
    {
        return Err("contact split did not report complete preservation".into());
    }
    let generated = job.join("parts/part-0001.pst");
    verify_exact_message_fidelity(source_messages, independent_messages(&generated)?)?;
    if independent_named_properties(&generated)? != source_named {
        return Err("contact named-property identity or payload changed".into());
    }
    if independent_folder_classes(&generated)?
        .get(&vec!["Contacts".to_owned()])
        .map(String::as_str)
        != Some("IPF.Contact")
    {
        return Err("generated contact folder is not IPF.Contact".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the external v042-appointment-source corpus case"]
fn milestone_0_4_2_appointments_roundtrip_through_libpff() -> Result<(), Box<dyn std::error::Error>>
{
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-appointment-source")
        .ok_or("manifest has no v042-appointment-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256 != case.sha256 {
        return Err("appointment source SHA-256 does not match its manifest".into());
    }

    let source_messages = independent_messages(&case.path)?;
    if source_messages.len() != 1
        || source_messages[0].folder_path != ["Calendar"]
        || source_messages[0].content.message_class.as_deref() != Some("IPM.Appointment")
        || source_messages[0].content.subject.as_deref() != Some("Appointment fidelity checkpoint")
        || source_messages[0].content.sender_name.is_some()
        || source_messages[0].content.sender_email.is_some()
        || !source_messages[0].content.recipients.is_empty()
        || !source_messages[0].complete
    {
        return Err("appointment source does not match the item contract".into());
    }
    let source_named = independent_named_properties(&case.path)?;
    let expected_named = [
        (0x02, 0x8205, 0x0003),
        (0x02, 0x8208, 0x001F),
        (0x02, 0x820D, 0x0040),
        (0x02, 0x820E, 0x0040),
        (0x02, 0x8213, 0x0003),
        (0x02, 0x8215, 0x000B),
        (0x02, 0x8217, 0x0003),
        (0x02, 0x8223, 0x000B),
        (0x08, 0x8501, 0x0003),
        (0x08, 0x8502, 0x0040),
        (0x08, 0x8503, 0x000B),
        (0x08, 0x8516, 0x0040),
        (0x08, 0x8517, 0x0040),
    ];
    if source_named.len() != expected_named.len()
        || !source_named.iter().zip(expected_named).all(
            |(property, (guid_first, expected_lid, expected_type))| {
                property.identity.guid
                    == [
                        guid_first, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00, 0x46,
                    ]
                    && property.identity.name == NamedPropertyName::Numeric(expected_lid)
                    && property.value_type == Some(expected_type)
            },
        )
    {
        return Err("appointment source named-property contract changed".into());
    }
    if independent_folder_classes(&case.path)?
        .get(&vec!["Calendar".to_owned()])
        .map(String::as_str)
        != Some("IPF.Appointment")
    {
        return Err("appointment source folder is not IPF.Appointment".into());
    }

    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "appointment split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if report["partial"].as_bool() != Some(false)
        || report["written_candidates"].as_u64() != Some(1)
        || report["parts"].as_array().map(Vec::len) != Some(1)
        || report["parts"][0]["omitted_folders"].as_u64() != Some(0)
        || report["parts"][0]["omitted_properties"].as_u64() != Some(0)
        || report["parts"][0]["omitted_attachments"].as_u64() != Some(0)
    {
        return Err("appointment split did not report complete preservation".into());
    }
    let generated = job.join("parts/part-0001.pst");
    verify_exact_message_fidelity(source_messages, independent_messages(&generated)?)?;
    if independent_named_properties(&generated)? != source_named {
        return Err("appointment named-property identity or payload changed".into());
    }
    if independent_folder_classes(&generated)?
        .get(&vec!["Calendar".to_owned()])
        .map(String::as_str)
        != Some("IPF.Appointment")
    {
        return Err("generated calendar folder is not IPF.Appointment".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the external v042-meeting-source corpus case"]
fn milestone_0_4_2_meetings_roundtrip_through_libpff() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-meeting-source")
        .ok_or("manifest has no v042-meeting-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256 != case.sha256 {
        return Err("meeting source SHA-256 does not match its manifest".into());
    }

    let source_messages = independent_messages(&case.path)?;
    if source_messages.len() != 1
        || source_messages[0].folder_path != ["Inbox"]
        || source_messages[0].content.message_class.as_deref()
            != Some("IPM.Schedule.Meeting.Request")
        || source_messages[0].content.subject.as_deref()
            != Some("Meeting request fidelity checkpoint")
        || source_messages[0].content.sender_name.as_deref() != Some("PSTForge Organizer")
        || source_messages[0].content.sender_email.as_deref() != Some("organizer@example.com")
        || source_messages[0].content.recipients.len() != 2
        || source_messages[0].content.recipients[0].recipient_type != Some(1)
        || source_messages[0].content.recipients[0]
            .email_address
            .as_deref()
            != Some("attendee@example.com")
        || source_messages[0].content.recipients[1].recipient_type != Some(2)
        || source_messages[0].content.recipients[1]
            .email_address
            .as_deref()
            != Some("optional@example.com")
        || !source_messages[0].complete
    {
        return Err("meeting source does not match the request contract".into());
    }
    let source_named = independent_named_properties(&case.path)?;
    let expected_named = [
        (0x02, 0x8201, 0x0003, None),
        (0x02, 0x8205, 0x0003, None),
        (0x02, 0x8208, 0x001F, None),
        (0x02, 0x820D, 0x0040, None),
        (0x02, 0x820E, 0x0040, None),
        (0x02, 0x8213, 0x0003, None),
        (0x02, 0x8215, 0x000B, None),
        (0x02, 0x8217, 0x0003, None),
        (0x02, 0x8218, 0x0003, None),
        (0x02, 0x8223, 0x000B, None),
        (0x08, 0x8501, 0x0003, None),
        (0x08, 0x8502, 0x0040, None),
        (0x08, 0x8503, 0x000B, None),
        (0x08, 0x8516, 0x0040, None),
        (0x08, 0x8517, 0x0040, None),
        (0x90, 0x0003, 0x0102, Some(56)),
        (0x90, 0x0023, 0x0102, Some(56)),
        (0x90, 0x0024, 0x001F, None),
        (0x90, 0x0026, 0x0003, None),
    ];
    if source_named.len() != expected_named.len()
        || !source_named.iter().zip(expected_named).all(
            |(property, (guid_first, expected_lid, expected_type, expected_len))| {
                let expected_guid = if guid_first == 0x90 {
                    [
                        0x90, 0xDA, 0xD8, 0x6E, 0x0B, 0x45, 0x1B, 0x10, 0x98, 0xDA, 0x00, 0xAA,
                        0x00, 0x3F, 0x13, 0x05,
                    ]
                } else {
                    [
                        guid_first, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00, 0x46,
                    ]
                };
                property.identity.guid == expected_guid
                    && property.identity.name == NamedPropertyName::Numeric(expected_lid)
                    && property.value_type == Some(expected_type)
                    && expected_len.is_none_or(|length| property.byte_len == length)
            },
        )
    {
        return Err("meeting source named-property contract changed".into());
    }
    if independent_folder_classes(&case.path)?
        .get(&vec!["Inbox".to_owned()])
        .map(String::as_str)
        != Some("IPF.Note")
    {
        return Err("meeting source Inbox is not IPF.Note".into());
    }

    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "meeting split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if report["partial"].as_bool() != Some(false)
        || report["written_candidates"].as_u64() != Some(1)
        || report["parts"].as_array().map(Vec::len) != Some(1)
        || report["parts"][0]["omitted_folders"].as_u64() != Some(0)
        || report["parts"][0]["omitted_properties"].as_u64() != Some(0)
        || report["parts"][0]["omitted_attachments"].as_u64() != Some(0)
    {
        return Err("meeting split did not report complete preservation".into());
    }
    let generated = job.join("parts/part-0001.pst");
    verify_exact_message_fidelity(source_messages, independent_messages(&generated)?)?;
    if independent_named_properties(&generated)? != source_named {
        return Err("meeting named-property identity or payload changed".into());
    }
    if independent_folder_classes(&generated)?
        .get(&vec!["Inbox".to_owned()])
        .map(String::as_str)
        != Some("IPF.Note")
    {
        return Err("generated meeting Inbox is not IPF.Note".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the external v042-pim-source corpus case"]
fn milestone_0_4_2_pim_items_roundtrip_through_libpff() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-pim-source")
        .ok_or("manifest has no v042-pim-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256 != case.sha256 {
        return Err("PIM source SHA-256 does not match its manifest".into());
    }

    let source_messages = independent_messages(&case.path)?;
    let item_contract = [
        ("IPM.Task", "Task fidelity checkpoint", "Tasks", None, None),
        (
            "IPM.StickyNote",
            "Sticky note fidelity checkpoint",
            "Notes",
            None,
            None,
        ),
        (
            "IPM.Post",
            "Post fidelity checkpoint",
            "Posts",
            Some("PSTForge Poster"),
            Some("poster@example.com"),
        ),
    ];
    if source_messages.len() != item_contract.len()
        || !item_contract
            .iter()
            .all(|(class, subject, folder, sender_name, sender_email)| {
                source_messages.iter().any(|message| {
                    message.folder_path == [*folder]
                        && message.content.message_class.as_deref() == Some(*class)
                        && message.content.subject.as_deref() == Some(*subject)
                        && message.content.sender_name.as_deref() == *sender_name
                        && message.content.sender_email.as_deref() == *sender_email
                        && message.content.recipients.is_empty()
                        && message.complete
                })
            })
    {
        return Err("PIM source does not match the three-item contract".into());
    }
    let source_named = independent_named_properties(&case.path)?;
    let expected_named = [
        ("IPM.Task", "Task fidelity checkpoint", 0x03, 0x8101, 0x0003),
        ("IPM.Task", "Task fidelity checkpoint", 0x03, 0x8102, 0x0005),
        ("IPM.Task", "Task fidelity checkpoint", 0x03, 0x8104, 0x0040),
        ("IPM.Task", "Task fidelity checkpoint", 0x03, 0x8105, 0x0040),
        ("IPM.Task", "Task fidelity checkpoint", 0x03, 0x811C, 0x000B),
        ("IPM.Task", "Task fidelity checkpoint", 0x03, 0x8126, 0x000B),
        (
            "IPM.StickyNote",
            "Sticky note fidelity checkpoint",
            0x0E,
            0x8B00,
            0x0003,
        ),
        (
            "IPM.StickyNote",
            "Sticky note fidelity checkpoint",
            0x0E,
            0x8B02,
            0x0003,
        ),
        (
            "IPM.StickyNote",
            "Sticky note fidelity checkpoint",
            0x0E,
            0x8B03,
            0x0003,
        ),
    ];
    if source_named.len() != expected_named.len()
        || !expected_named.iter().all(
            |(class, subject, guid_first, expected_lid, expected_type)| {
                source_named.iter().any(|property| {
                    property.owner.message_class.as_deref() == Some(*class)
                        && property.owner.subject.as_deref() == Some(*subject)
                        && property.identity.guid
                            == [
                                *guid_first,
                                0x20,
                                0x06,
                                0x00,
                                0x00,
                                0x00,
                                0x00,
                                0x00,
                                0xC0,
                                0x00,
                                0x00,
                                0x00,
                                0x00,
                                0x00,
                                0x00,
                                0x46,
                            ]
                        && property.identity.name == NamedPropertyName::Numeric(*expected_lid)
                        && property.value_type == Some(*expected_type)
                })
            },
        )
    {
        return Err(format!("PIM source named-property contract changed: {source_named:?}").into());
    }
    let source_classes = independent_folder_classes(&case.path)?;
    for (folder, class) in [
        ("Tasks", "IPF.Task"),
        ("Notes", "IPF.StickyNote"),
        ("Posts", "IPF.Note"),
    ] {
        if source_classes
            .get(&vec![folder.to_owned()])
            .map(String::as_str)
            != Some(class)
        {
            return Err(format!("PIM source folder {folder:?} is not {class}").into());
        }
    }

    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "PIM split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if report["partial"].as_bool() != Some(false)
        || report["written_candidates"].as_u64() != Some(3)
        || report["parts"].as_array().map(Vec::len) != Some(1)
        || report["parts"][0]["omitted_folders"].as_u64() != Some(0)
        || report["parts"][0]["omitted_properties"].as_u64() != Some(0)
        || report["parts"][0]["omitted_attachments"].as_u64() != Some(0)
    {
        return Err("PIM split did not report complete preservation".into());
    }
    let generated = job.join("parts/part-0001.pst");
    verify_exact_message_fidelity(source_messages, independent_messages(&generated)?)?;
    if independent_named_properties(&generated)? != source_named {
        return Err("PIM named-property identity or payload changed".into());
    }
    if independent_folder_classes(&generated)? != source_classes {
        return Err("PIM folder classes changed during the split".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_4_real_pst_splits_deterministically_without_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let cases = manifest
        .cases
        .iter()
        .filter(|case| case.milestone_0_4)
        .collect::<Vec<_>>();
    if cases.is_empty() {
        return Err("manifest has no milestone_0_4 split case".into());
    }

    for case in cases {
        let before_metadata = fs::metadata(&case.path)?;
        let before = pstforge_core::SourceFile::open(&case.path)?
            .identity()
            .clone();
        if before.sha256 != case.sha256 {
            return Err(format!("{} SHA-256 does not match its manifest", case.name).into());
        }
        let source_messages = independent_messages(&case.path)?;
        let incomplete_source_messages = source_messages
            .iter()
            .filter(|message| !message.complete)
            .count();
        let mut runs = Vec::new();
        for _ in 0..2 {
            let directory = tempfile::tempdir()?;
            let job = directory.path().join("job");
            let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
                .arg("split")
                .arg(&case.path)
                .arg("--output")
                .arg(&job)
                .arg("--max-pst-size")
                .arg(case.milestone_0_4_max_pst_bytes.to_string())
                .arg("--json")
                .arg("--color")
                .arg("never")
                .output()?;
            if !output.status.success() && output.status.code() != Some(1) {
                return Err(format!(
                    "split failed for {}: {}",
                    case.name,
                    String::from_utf8_lossy(&output.stderr)
                )
                .into());
            }
            let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            if incomplete_source_messages != 0
                && (report["partial"].as_bool() != Some(true)
                    || report["recovery"]["partial_candidates"]
                        .as_u64()
                        .unwrap_or_default()
                        == 0)
            {
                return Err(format!(
                    "{} did not report libpff attachment-count uncertainty as partial",
                    case.name
                )
                .into());
            }
            if report["maximum_pst_bytes"].as_u64() != Some(case.milestone_0_4_max_pst_bytes)
                || report["recovery"]["source"]["sha256"].as_str() != Some(case.sha256.as_str())
                || report["recovery"]["source_unchanged"].as_bool() != Some(true)
            {
                return Err(format!("{} split report identity mismatch", case.name).into());
            }
            let written = report["written_candidates"].as_u64().unwrap_or_default();
            let committed = report["recovery"]["committed_candidates"]
                .as_u64()
                .unwrap_or_default();
            let unsupported = report["recovery"]["unsupported_candidates"]
                .as_u64()
                .unwrap_or_default();
            if written.saturating_add(unsupported) != committed {
                return Err(format!("{} split candidate accounting mismatch", case.name).into());
            }
            if written < case.minimum_messages {
                return Err(format!("{} wrote fewer than the manifest minimum", case.name).into());
            }
            let parts = report["parts"]
                .as_array()
                .ok_or("split report parts is not an array")?;
            if parts.len() < 2 {
                return Err(format!("{} did not exercise a part boundary", case.name).into());
            }
            let mut identities = Vec::new();
            let mut output_messages = 0_u64;
            let mut output_fingerprints = Vec::new();
            for part in parts {
                let filename = part["filename"].as_str().ok_or("part filename is absent")?;
                let byte_len = part["byte_len"].as_u64().ok_or("part length is absent")?;
                let sha256 = part["sha256"].as_str().ok_or("part hash is absent")?;
                let oversize = part["oversize"].as_bool().ok_or("oversize is absent")?;
                if oversize && !case.milestone_0_4_allow_oversize {
                    return Err(
                        format!("{} unexpectedly required an oversize part", case.name).into(),
                    );
                }
                if !oversize && byte_len > case.milestone_0_4_max_pst_bytes {
                    return Err(format!("{} published an over-limit normal part", case.name).into());
                }
                let path = job.join("parts").join(filename);
                let identity = pstforge_core::SourceFile::open(&path)?.identity().clone();
                if identity.size_bytes != byte_len || identity.sha256 != sha256 {
                    return Err(format!("{} part identity mismatch", case.name).into());
                }
                let inventory = pstforge_core::verify(&path)?;
                output_messages = output_messages.saturating_add(inventory.inventory.normal_items);
                let part_fingerprints = independent_messages(&path)?;
                let (replicated_leaf_folders, replicated_all_folders) =
                    replicated_source_folder_counts(&source_messages, &part_fingerprints)?;
                output_fingerprints.extend(part_fingerprints);
                let store = pstforge_pst::open_store(&path)?;
                let record_key = lower_hex(store.properties().record_key()?.record_key());
                let sidecar_name = format!("{}.json", filename.trim_end_matches(".pst"));
                let sidecar_bytes = fs::read(job.join("parts").join(sidecar_name))?;
                let sidecar: pstforge_job::PartSidecar = serde_json::from_slice(&sidecar_bytes)?;
                let expected_inventory_folders =
                    replicated_all_folders.saturating_add(WRITER_MANDATORY_FOLDER_COUNT);
                if sidecar.folder_count != replicated_leaf_folders
                    || inventory.inventory.folders != expected_inventory_folders
                {
                    return Err(format!(
                        "{} folder accounting mismatch: sidecar={}, source leaves={}, inventory={}, expected inventory={}",
                        case.name,
                        sidecar.folder_count,
                        replicated_leaf_folders,
                        inventory.inventory.folders,
                        expected_inventory_folders
                    )
                    .into());
                }
                if sidecar.schema_version != "1.0.0"
                    || sidecar.producer_version != env!("CARGO_PKG_VERSION")
                    || u64::from(sidecar.index) != part["index"].as_u64().unwrap_or_default()
                    || sidecar.filename != filename
                    || sidecar.byte_len != byte_len
                    || sidecar.sha256 != sha256
                    || sidecar.oversize != oversize
                    || Some(sidecar.folder_count) != part["folder_count"].as_u64()
                    || Some(sidecar.message_count) != part["message_count"].as_u64()
                    || sidecar.message_count != inventory.inventory.normal_items
                    || Some(sidecar.partial) != part["partial"].as_bool()
                    || Some(sidecar.omitted_properties) != part["omitted_properties"].as_u64()
                    || Some(sidecar.omitted_attachments) != part["omitted_attachments"].as_u64()
                    || sidecar.store_record_key != record_key
                {
                    return Err(format!("{} part sidecar mismatch", case.name).into());
                }
                identities.push((
                    filename.to_owned(),
                    sha256.to_owned(),
                    byte_len,
                    sidecar_bytes,
                ));
            }
            if output_messages != written {
                return Err(format!("{} generated message count mismatch", case.name).into());
            }
            verify_exact_message_fidelity(source_messages.clone(), output_fingerprints)?;
            for entry in fs::read_dir(job.join(".pstforge/partial"))? {
                let entry = entry?;
                let name = entry.file_name();
                if !entry.file_type()?.is_dir()
                    || !name.to_string_lossy().starts_with(".pstforge-")
                    || fs::read_dir(entry.path())?.next().is_some()
                {
                    return Err(format!(
                        "{} left nonempty or unrecognized publication scratch",
                        case.name
                    )
                    .into());
                }
            }
            drop(pstforge_job::DurableCatalogSink::open(&job)?);
            let initial_attempts = report["recovery"]["worker_attempts"]
                .as_u64()
                .ok_or("split report worker attempts are absent")?;
            let initial_blob_count = report["recovery"]["blob_count"]
                .as_u64()
                .ok_or("split report blob count is absent")?;
            let initial_blob_bytes = report["recovery"]["blob_bytes"]
                .as_u64()
                .ok_or("split report blob bytes are absent")?;
            let initial_parts = report["parts"].clone();
            let resume = Command::new(env!("CARGO_BIN_EXE_pstforge"))
                .arg("split")
                .arg(&case.path)
                .arg("--output")
                .arg(&job)
                .arg("--max-pst-size")
                .arg(case.milestone_0_4_max_pst_bytes.to_string())
                .arg("--resume")
                .arg("--json")
                .arg("--color")
                .arg("never")
                .output()?;
            if !resume.status.success() && resume.status.code() != Some(1) {
                return Err(format!(
                    "completed resume failed for {}: {}",
                    case.name,
                    String::from_utf8_lossy(&resume.stderr)
                )
                .into());
            }
            let resumed: serde_json::Value = serde_json::from_slice(&resume.stdout)?;
            if resumed["resumed"].as_bool() != Some(true)
                || resumed["parts"] != initial_parts
                || resumed["recovery"]["worker_attempts"].as_u64() != Some(initial_attempts)
                || resumed["recovery"]["blob_count"].as_u64() != Some(initial_blob_count)
                || resumed["recovery"]["blob_bytes"].as_u64() != Some(initial_blob_bytes)
            {
                return Err(format!(
                    "{} completed resume changed parts, metrics, or restarted parsing",
                    case.name
                )
                .into());
            }
            let ledger = job.join(".pstforge/job.sqlite3");
            let before_mismatch = Sha256::digest(fs::read(&ledger)?);
            let mismatch = Command::new(env!("CARGO_BIN_EXE_pstforge"))
                .arg("split")
                .arg(&case.path)
                .arg("--output")
                .arg(&job)
                .arg("--max-pst-size")
                .arg(
                    case.milestone_0_4_max_pst_bytes
                        .saturating_add(1)
                        .to_string(),
                )
                .arg("--resume")
                .arg("--json")
                .arg("--color")
                .arg("never")
                .output()?;
            if mismatch.status.code() != Some(4) {
                return Err(format!("{} accepted a mismatched resume", case.name).into());
            }
            if Sha256::digest(fs::read(&ledger)?) != before_mismatch {
                return Err(format!("{} mismatch validation mutated its ledger", case.name).into());
            }
            runs.push(identities);
        }
        if runs[0] != runs[1] {
            return Err(format!("{} split output is not deterministic", case.name).into());
        }
        let after_metadata = fs::metadata(&case.path)?;
        let after = pstforge_core::SourceFile::open(&case.path)?
            .identity()
            .clone();
        if before != after
            || before_metadata.len() != after_metadata.len()
            || modified_ns(&before_metadata)? != modified_ns(&after_metadata)?
            || accessed_ns(&before_metadata) != accessed_ns(&after_metadata)
        {
            return Err(format!("{} changed during deterministic splitting", case.name).into());
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_4_1_interruption_and_sigkill_resume_without_orphan_worker()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .filter(|case| case.milestone_0_4)
        .min_by_key(|case| fs::metadata(&case.path).map_or(u64::MAX, |metadata| metadata.len()))
        .ok_or("manifest has no milestone_0_4 split case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();

    for signal in [rustix::process::Signal::TERM, rustix::process::Signal::KILL] {
        let directory = tempfile::tempdir()?;
        let job = directory.path().join("job");
        let mut child = Command::new(env!("CARGO_BIN_EXE_pstforge"))
            .arg("split")
            .arg(&case.path)
            .arg("--output")
            .arg(&job)
            .arg("--max-pst-size")
            .arg(case.milestone_0_4_max_pst_bytes.to_string())
            .arg("--json")
            .arg("--color")
            .arg("never")
            .env("PSTFORGE_TEST_STALL_AFTER_CANDIDATES", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let supervisor_id = child.id();
        let deadline = Instant::now() + Duration::from_secs(10);
        let worker_id = loop {
            let children_path = format!("/proc/{supervisor_id}/task/{supervisor_id}/children");
            if let Ok(children) = fs::read_to_string(children_path)
                && let Some(worker) = children
                    .split_ascii_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u32>().ok())
            {
                break worker;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                return Err("split worker did not start before signal deadline".into());
            }
            thread::sleep(Duration::from_millis(25));
        };
        while !job.join(".pstforge/job.sqlite3").is_file() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                return Err("split ledger did not become durable before signal deadline".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        let supervisor_pid = i32::try_from(supervisor_id)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or("supervisor PID is out of range")?;
        rustix::process::kill_process(supervisor_pid, signal)?;
        let stopped = child.wait_with_output()?;
        if signal == rustix::process::Signal::TERM {
            if stopped.status.code() != Some(130) {
                return Err("SIGTERM did not produce interrupted status".into());
            }
            let report: serde_json::Value = serde_json::from_slice(&stopped.stdout)?;
            if report["recovery"]["interrupted"].as_bool() != Some(true) {
                return Err("SIGTERM report did not record interruption".into());
            }
        } else if stopped.status.success() {
            return Err("SIGKILL unexpectedly reported success".into());
        }
        let worker_deadline = Instant::now() + Duration::from_secs(5);
        while PathBuf::from(format!("/proc/{worker_id}")).exists() {
            if Instant::now() >= worker_deadline {
                return Err("parser worker outlived its killed supervisor".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        let resumed = Command::new(env!("CARGO_BIN_EXE_pstforge"))
            .arg("split")
            .arg(&case.path)
            .arg("--output")
            .arg(&job)
            .arg("--max-pst-size")
            .arg(case.milestone_0_4_max_pst_bytes.to_string())
            .arg("--resume")
            .arg("--json")
            .arg("--color")
            .arg("never")
            .output()?;
        if !resumed.status.success() && resumed.status.code() != Some(1) {
            return Err(String::from_utf8_lossy(&resumed.stderr).into_owned().into());
        }
        let report: serde_json::Value = serde_json::from_slice(&resumed.stdout)?;
        if report["parts"].as_array().is_none_or(Vec::is_empty)
            || report["recovery"]["source_unchanged"].as_bool() != Some(true)
        {
            return Err("resumed split did not finalize source-safe parts".into());
        }
    }
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg(case.milestone_0_4_max_pst_bytes.to_string())
        .arg("--json")
        .arg("--color")
        .arg("never")
        .env("PSTFORGE_TEST_PAUSE_AFTER_PART_MS", "5000")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let first_part = job.join("parts/part-0001.pst");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !first_part.is_file() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err("split did not publish its first part before signal deadline".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    let first_part_hash = Sha256::digest(fs::read(&first_part)?);
    let supervisor_pid = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or("supervisor PID is out of range")?;
    rustix::process::kill_process(supervisor_pid, rustix::process::Signal::TERM)?;
    let stopped = child.wait_with_output()?;
    if stopped.status.code() != Some(130) {
        return Err("post-publication SIGTERM did not produce interrupted status".into());
    }
    let interrupted: serde_json::Value = serde_json::from_slice(&stopped.stdout)?;
    if interrupted["parts"].as_array().is_none_or(Vec::is_empty)
        || interrupted["recovery"]["interrupted"].as_bool() != Some(true)
        || Sha256::digest(fs::read(&first_part)?) != first_part_hash
    {
        return Err("post-publication interruption lost its finalized part".into());
    }
    let resumed = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg(case.milestone_0_4_max_pst_bytes.to_string())
        .arg("--resume")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !resumed.status.success() && resumed.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&resumed.stderr).into_owned().into());
    }
    let resumed: serde_json::Value = serde_json::from_slice(&resumed.stdout)?;
    if resumed["parts"]
        .as_array()
        .is_none_or(|parts| parts.len() < 2)
        || Sha256::digest(fs::read(&first_part)?) != first_part_hash
    {
        return Err("resume did not preserve and continue after finalized part".into());
    }

    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .env("PSTFORGE_TEST_PAUSE_AT_PREFILTER_MS", "5000")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let prefilter_marker = job.join(".pstforge/partial/prefilter-test-marker.partial");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !prefilter_marker.is_file() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err("split did not enter candidate prefilter before signal deadline".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    let supervisor_pid = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or("supervisor PID is out of range")?;
    rustix::process::kill_process(supervisor_pid, rustix::process::Signal::TERM)?;
    let stopped = child.wait_with_output()?;
    if stopped.status.code() != Some(130) {
        return Err("candidate prefilter SIGTERM did not produce interrupted status".into());
    }
    let interrupted: serde_json::Value = serde_json::from_slice(&stopped.stdout)?;
    if interrupted["recovery"]["interrupted"].as_bool() != Some(true) {
        return Err("candidate prefilter interruption was not reported".into());
    }
    let resumed = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--resume")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !resumed.status.success() && resumed.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&resumed.stderr).into_owned().into());
    }
    if prefilter_marker.exists() {
        return Err("resumed split retained its prefilter interruption marker".into());
    }

    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .env("PSTFORGE_TEST_LONG_CLEANUP_SQL", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let cleanup_marker = job.join(".pstforge/partial/cleanup-test-marker.partial");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !cleanup_marker.is_file() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err("split did not enter cleanup SQL before signal deadline".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    let supervisor_pid = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or("supervisor PID is out of range")?;
    rustix::process::kill_process(supervisor_pid, rustix::process::Signal::TERM)?;
    let stopped = child.wait_with_output()?;
    if stopped.status.code() != Some(130) {
        return Err("cleanup SQL SIGTERM did not produce interrupted status".into());
    }
    let interrupted: serde_json::Value = serde_json::from_slice(&stopped.stdout)?;
    if interrupted["recovery"]["interrupted"].as_bool() != Some(true) {
        return Err("cleanup SQL interruption was not reported".into());
    }
    let resumed = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--resume")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !resumed.status.success() && resumed.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&resumed.stderr).into_owned().into());
    }
    if cleanup_marker.exists() {
        return Err("resumed cleanup retained its interruption marker".into());
    }
    if pstforge_core::SourceFile::open(&case.path)?.identity() != &before {
        return Err("interruption qualification changed the source".into());
    }
    Ok(())
}

#[test]
fn validator_process_group_dies_when_its_supervisor_disappears()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let wrapper_file = directory.path().join("wrapper.pid");
    let child_file = directory.path().join("child.pid");
    let launcher = Command::new("sh")
        .arg("-c")
        .arg(
            r#"
setsid "$PSTFORGE_BIN" __validator "$$" pffinfo > /dev/null 2>&1 &
wrapper=$!
printf '%s\n' "$wrapper" > "$1"
child_file=$2
attempt=0
while [ "$attempt" -lt 400 ]; do
    children=$(cat "/proc/$wrapper/task/$wrapper/children" 2>/dev/null || true)
    if [ -n "$children" ]; then
        set -- $children
        printf '%s\n' "$1" > "$child_file"
        exit 0
    fi
    attempt=$((attempt + 1))
    sleep 0.01
done
exit 1
"#,
        )
        .arg("validator-launcher")
        .arg(&wrapper_file)
        .arg(&child_file)
        .env("PSTFORGE_BIN", env!("CARGO_BIN_EXE_pstforge"))
        .env("PSTFORGE_TEST_STALL_VALIDATOR", "1")
        .status()?;
    if !launcher.success() {
        return Err("validator descendant did not start before its supervisor exited".into());
    }
    let wrapper_id: u32 = fs::read_to_string(&wrapper_file)?.trim().parse()?;
    let child_id: u32 = fs::read_to_string(&child_file)?.trim().parse()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while PathBuf::from(format!("/proc/{wrapper_id}")).exists()
        || PathBuf::from(format!("/proc/{child_id}")).exists()
    {
        if Instant::now() >= deadline {
            return Err("validator process group outlived its supervisor".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_external_recovery_spools_without_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    if manifest.schema_version != 1 {
        return Err(format!("unsupported corpus schema {}", manifest.schema_version).into());
    }
    let cases: Vec<&Case> = manifest
        .cases
        .iter()
        .filter(|case| case.milestone_0_3 || case.classification == "damaged")
        .collect();
    if cases.is_empty() {
        return Err("manifest has no milestone_0_3 or damaged cases".into());
    }

    for case in cases {
        let before_metadata = fs::metadata(&case.path)?;
        let before_hash = pstforge_core::SourceFile::open(&case.path)?
            .identity()
            .sha256
            .clone();
        if before_hash != case.sha256 {
            return Err(format!("{} SHA-256 does not match its manifest", case.name).into());
        }
        let directory = tempfile::tempdir()?;
        let job = directory.path().join("job");
        let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
            .arg("recover")
            .arg(&case.path)
            .arg("--output")
            .arg(&job)
            .arg("--json")
            .arg("--color")
            .arg("never")
            .output()?;
        if !output.status.success() && output.status.code() != Some(1) {
            return Err(format!(
                "recover failed for {}: {}",
                case.name,
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let normal = report["normal_items"].as_u64().unwrap_or_default();
        let recovered = report["recovered_items"].as_u64().unwrap_or_default();
        let orphan = report["orphan_items"].as_u64().unwrap_or_default();
        let committed = report["committed_candidates"].as_u64().unwrap_or_default();
        if normal < case.minimum_messages
            || recovered < case.minimum_recovered_items
            || orphan < case.minimum_orphan_items
            || committed != normal + recovered + orphan
        {
            return Err(format!(
                "{} recovery totals violate manifest expectations",
                case.name
            )
            .into());
        }
        if !job.join(".pstforge/job.sqlite3").is_file() {
            return Err(format!("{} did not produce a durable job ledger", case.name).into());
        }

        let after_metadata = fs::metadata(&case.path)?;
        let after_hash = pstforge_core::SourceFile::open(&case.path)?
            .identity()
            .sha256
            .clone();
        if before_hash != after_hash
            || before_metadata.len() != after_metadata.len()
            || modified_ns(&before_metadata)? != modified_ns(&after_metadata)?
            || accessed_ns(&before_metadata) != accessed_ns(&after_metadata)
        {
            return Err(format!("{} changed during recovery", case.name).into());
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_aggressive_recovery_is_distinct_and_non_mutating()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .min_by_key(|case| {
            fs::metadata(&case.path)
                .map(|metadata| metadata.len())
                .unwrap_or(u64::MAX)
        })
        .ok_or("manifest has no recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if before.sha256 != case.sha256 {
        return Err(format!("{} SHA-256 does not match its manifest", case.name).into());
    }
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--recovery")
        .arg("aggressive")
        .arg("--json")
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["mode"], "aggressive");
    let committed = report["committed_candidates"].as_u64().unwrap_or_default();
    let normal = report["normal_items"].as_u64().unwrap_or_default();
    let recovered = report["recovered_items"].as_u64().unwrap_or_default();
    let orphan = report["orphan_items"].as_u64().unwrap_or_default();
    let fragments = report["fragment_items"].as_u64().unwrap_or_default();
    assert_eq!(committed, normal + recovered + orphan + fragments);
    let sink = pstforge_job::DurableCatalogSink::open(&job)?;
    let summary = sink.summary()?;
    assert_eq!(summary.committed_candidates, committed);
    assert_eq!(summary.recovered_candidates, recovered);
    assert_eq!(summary.orphan_candidates, orphan);
    assert_eq!(summary.fragment_candidates, fragments);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_worker_abort_replays_committed_candidates()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 1
        })
        .ok_or("manifest has no multi-message recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_ABORT_AFTER_CANDIDATES", "1")
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 2);
    assert_eq!(report["worker_failures"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_persistent_worker_abort_is_bounded_and_partial()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 1
        })
        .ok_or("manifest has no multi-message recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_ABORT_EVERY_ATTEMPT_AFTER_CANDIDATES", "1")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 4);
    assert_eq!(report["worker_failures"], 4);
    assert_eq!(report["committed_candidates"], 1);
    assert_eq!(report["issues"], 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_worker_stall_is_killed_and_replayed() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 1
        })
        .ok_or("manifest has no multi-message recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_STALL_AFTER_CANDIDATES", "1")
        .env("PSTFORGE_TEST_STALL_TIMEOUT_MS", "1000")
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 2);
    assert_eq!(report["worker_failures"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_repeated_unit_crash_is_isolated() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 2
        })
        .ok_or("manifest has no recovery case with at least three messages")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_ABORT_ON_UNIT_ORDINAL", "2")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 2);
    assert_eq!(report["worker_failures"], 1);
    assert_eq!(report["isolated_units"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert!(report["issues"].as_u64().unwrap_or_default() >= 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_replayed_candidate_does_not_prevent_unit_isolation()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 2
        })
        .ok_or("manifest has no recovery case with at least three messages")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_ABORT_INSIDE_UNIT_AFTER_CANDIDATES", "1")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 2);
    assert_eq!(report["worker_failures"], 1);
    assert_eq!(report["isolated_units"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_sigsegv_is_contained_and_isolated() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.milestone_0_3 || case.classification == "damaged")
        .ok_or("manifest has no damaged recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_SEGV_ON_UNIT_ORDINAL", "2")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 2);
    assert_eq!(report["worker_failures"], 1);
    assert_eq!(report["isolated_units"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 1);
    assert!(job.join(".pstforge/job.sqlite3").is_file());
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_parser_error_after_commit_is_contained_without_rescan()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| {
            (case.milestone_0_3 || case.classification == "damaged") && case.minimum_messages > 1
        })
        .ok_or("manifest has no multi-message recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    let directory = tempfile::tempdir()?;
    let job = directory.path().join("job");
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("recover")
        .arg(&case.path)
        .arg("--output")
        .arg(&job)
        .arg("--json")
        .env("PSTFORGE_TEST_PARSER_ERROR_AFTER_CANDIDATES", "1")
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["worker_attempts"], 1);
    assert_eq!(report["worker_failures"], 1);
    assert!(report["committed_candidates"].as_u64().unwrap_or_default() >= 1);
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_3_sigint_and_sigterm_leave_durable_partial_jobs()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.milestone_0_3 || case.classification == "damaged")
        .ok_or("manifest has no damaged recovery case")?;
    let before = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    for signal in [rustix::process::Signal::INT, rustix::process::Signal::TERM] {
        let directory = tempfile::tempdir()?;
        let job = directory.path().join("job");
        let mut child = Command::new(env!("CARGO_BIN_EXE_pstforge"))
            .arg("recover")
            .arg(&case.path)
            .arg("--output")
            .arg(&job)
            .arg("--json")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !job.join(".pstforge/job.sqlite3").is_file() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                return Err("recovery job did not start before signal deadline".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        thread::sleep(Duration::from_millis(500));
        let pid = i32::try_from(child.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or("child PID is out of range")?;
        rustix::process::kill_process(pid, signal)?;
        let output = child.wait_with_output()?;
        assert_eq!(output.status.code(), Some(130));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(report["interrupted"], true);
        assert!(report["committed_candidates"].as_u64().unwrap_or_default() > 0);
        assert!(job.join(".pstforge/job.sqlite3").is_file());
    }
    assert_eq!(
        pstforge_core::SourceFile::open(&case.path)?.identity(),
        &before
    );
    Ok(())
}

#[test]
#[ignore = "requires PSTFORGE_CORPUS_MANIFEST with external real PST files"]
fn milestone_0_1_external_psts_are_inspected_without_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest_text = fs::read_to_string(&manifest_path)?;
    let manifest: Manifest = toml::from_str(&manifest_text)?;
    if manifest.schema_version != 1 {
        return Err(format!("unsupported corpus schema {}", manifest.schema_version).into());
    }
    let cases: Vec<&Case> = manifest
        .cases
        .iter()
        .filter(|case| case.milestone_0_1 || case.milestone_0_1_1)
        .collect();
    if cases.is_empty() {
        return Err("manifest has no milestone_0_1 cases".into());
    }

    for case in cases {
        if !matches!(
            case.classification.as_str(),
            "healthy_ansi" | "healthy_unicode"
        ) {
            return Err(format!("{} is not classified as a healthy PST", case.name).into());
        }
        let before_metadata = fs::metadata(&case.path)?;
        let before_hash = pstforge_core::SourceFile::open(&case.path)?
            .identity()
            .sha256
            .clone();
        if before_hash != case.sha256 {
            return Err(format!("{} SHA-256 does not match its manifest", case.name).into());
        }

        let info = run_json("info", case)?;
        if info["source"]["sha256"] != case.sha256 {
            return Err(format!("{} info returned a different SHA-256", case.name).into());
        }
        let verify = run_json("verify", case)?;
        let folders = verify["inventory"]["folders"].as_u64().unwrap_or_default();
        let messages = verify["inventory"]["normal_items"]
            .as_u64()
            .unwrap_or_default();
        if folders < case.minimum_folders || messages < case.minimum_messages {
            return Err(format!("{} inventory is below manifest minimums", case.name).into());
        }
        if case.milestone_0_1_1 {
            let recipients = verify["inventory"]["recipients"]
                .as_u64()
                .unwrap_or_default();
            let attachments = verify["inventory"]["attachments"]
                .as_u64()
                .unwrap_or_default();
            let properties = verify["inventory"]["raw_properties"]
                .as_u64()
                .unwrap_or_default();
            let peak = verify["inventory"]["peak_stream_chunk_bytes"]
                .as_u64()
                .unwrap_or(u64::MAX);
            if recipients < case.minimum_recipients
                || attachments < case.minimum_attachments
                || properties < case.minimum_raw_properties
                || peak > case.maximum_peak_stream_chunk_bytes
            {
                return Err(format!("{} catalog is outside manifest invariants", case.name).into());
            }
        }

        let after_metadata = fs::metadata(&case.path)?;
        let after_hash = pstforge_core::SourceFile::open(&case.path)?
            .identity()
            .sha256
            .clone();
        if before_hash != after_hash
            || before_metadata.len() != after_metadata.len()
            || modified_ns(&before_metadata)? != modified_ns(&after_metadata)?
            || accessed_ns(&before_metadata) != accessed_ns(&after_metadata)
        {
            return Err(format!("{} changed during inspection", case.name).into());
        }
    }
    Ok(())
}

fn default_peak_chunk_limit() -> u64 {
    65_536
}

fn run_json(command: &str, case: &Case) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg(command)
        .arg(&case.path)
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !(output.status.success() || command == "verify" && output.status.code() == Some(1)) {
        return Err(format!(
            "{} failed for {}: {}",
            command,
            case.name,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn modified_ns(metadata: &fs::Metadata) -> Result<std::time::SystemTime, std::io::Error> {
    metadata.modified()
}

fn accessed_ns(metadata: &fs::Metadata) -> (i64, i64) {
    (metadata.atime(), metadata.atime_nsec())
}
