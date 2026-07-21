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
    PropertyOwner, TraversalOrder,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const WRITER_MANDATORY_FOLDER_COUNT: u64 = 5;
const NID_IPM_SUBTREE: u32 = 0x8022;
type MatchedSourceMessages = (Vec<Vec<String>>, usize);
type FolderIdentity = Vec<(String, u32)>;
type RepairedMessageCatalog = (Vec<MessageFingerprint>, BTreeSet<FolderIdentity>);

#[derive(Default)]
struct WriterOrderSink {
    message_stack: Vec<u32>,
    attachments: Vec<(u32, u32)>,
    nested_messages: u64,
    attachment_payloads: u64,
    attachment_properties: u64,
    message_properties: u64,
}

impl CatalogSink for WriterOrderSink {
    fn traversal_order(&self) -> TraversalOrder {
        TraversalOrder::Writer
    }

    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
        match event {
            CatalogEvent::MessageStart {
                id,
                parent_message_id,
                parent_attachment_index,
                ..
            } => {
                if let (Some(parent), Some(index)) = (parent_message_id, parent_attachment_index) {
                    if self.message_stack.last().copied() != Some(parent)
                        || !self
                            .attachments
                            .last()
                            .is_some_and(|active| active.0 == parent && active.1 == index)
                    {
                        return Err("embedded message is outside its parent attachment".to_owned());
                    }
                    self.nested_messages = self.nested_messages.saturating_add(1);
                } else if !self.message_stack.is_empty() {
                    return Err("top-level message was nested".to_owned());
                }
                self.message_stack.push(id);
            }
            CatalogEvent::MessageEnd { id, .. } => {
                if self.message_stack.pop() != Some(id) {
                    return Err("message end is not properly nested".to_owned());
                }
            }
            CatalogEvent::AttachmentStart {
                message_id, index, ..
            } => {
                if self.message_stack.last().copied() != Some(message_id)
                    || self
                        .attachments
                        .last()
                        .is_some_and(|active| active.0 == message_id)
                {
                    return Err("attachment start is outside its message".to_owned());
                }
                self.attachments.push((message_id, index));
            }
            CatalogEvent::AttachmentData {
                message_id, index, ..
            } => {
                let Some(active) = self.attachments.last() else {
                    return Err("attachment payload has no active attachment".to_owned());
                };
                if active.0 != message_id || active.1 != index {
                    return Err("attachment payload is outside its attachment".to_owned());
                }
                self.attachment_payloads = self.attachment_payloads.saturating_add(1);
            }
            CatalogEvent::PropertyStart(descriptor) => match descriptor.owner {
                PropertyOwner::Attachment { message_id, index } => {
                    let Some(active) = self.attachments.last() else {
                        return Err("attachment property has no active attachment".to_owned());
                    };
                    if active.0 != message_id
                        || active.1 != index
                        || self.message_stack.last().copied() != Some(message_id)
                    {
                        return Err("attachment property is outside writer order".to_owned());
                    }
                    self.attachment_properties = self.attachment_properties.saturating_add(1);
                }
                PropertyOwner::Message(message_id) => {
                    if self.message_stack.last().copied() != Some(message_id)
                        || self
                            .attachments
                            .last()
                            .is_some_and(|active| active.0 == message_id)
                    {
                        return Err("message property preceded attachment completion".to_owned());
                    }
                    self.message_properties = self.message_properties.saturating_add(1);
                }
                _ => {}
            },
            CatalogEvent::AttachmentEnd { message_id, index }
            | CatalogEvent::AttachmentAbort { message_id, index } => {
                if self.attachments.pop() != Some((message_id, index)) {
                    return Err("attachment end does not match its start".to_owned());
                }
            }
            _ => {}
        }
        Ok(())
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PlacementFingerprint {
    folder_name: String,
    folder_parent_is_store_root: bool,
    associated: bool,
    message_class: String,
    properties: Vec<PropertyFingerprint>,
}

#[derive(Default)]
struct PlacementSink {
    folders: BTreeMap<u32, (Option<u32>, Option<String>)>,
    messages: BTreeMap<u32, (u32, bool, String)>,
    active: BTreeMap<(u32, u32, u32), ActivePlacementProperty>,
    properties: BTreeMap<u32, Vec<PropertyFingerprint>>,
}

struct ActivePlacementProperty {
    id: u32,
    value_type: Option<u32>,
    hasher: Sha256,
    byte_len: u64,
}

impl CatalogSink for PlacementSink {
    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
        match event {
            CatalogEvent::Folder {
                id,
                parent_id,
                name,
                ..
            } => {
                self.folders.insert(id, (parent_id, name));
            }
            CatalogEvent::MessageStart {
                id,
                folder_id: Some(folder_id),
                associated,
                message_class: Some(message_class),
                ..
            } => {
                self.messages
                    .insert(id, (folder_id, associated, message_class));
            }
            CatalogEvent::PropertyStart(descriptor)
                if matches!(descriptor.owner, PropertyOwner::Message(_))
                    && descriptor.entry_type.is_some_and(|id| {
                        matches!(id, 0x0E07 | 0x3001) || (0x6000..=0x6002).contains(&id)
                    }) =>
            {
                let PropertyOwner::Message(message_id) = descriptor.owner else {
                    return Err("placement property owner changed during matching".to_owned());
                };
                self.active.insert(
                    (
                        message_id,
                        descriptor.record_set_index,
                        descriptor.entry_index,
                    ),
                    ActivePlacementProperty {
                        id: descriptor.entry_type.unwrap_or_default(),
                        value_type: descriptor.value_type,
                        hasher: Sha256::new(),
                        byte_len: 0,
                    },
                );
            }
            CatalogEvent::PropertyData { descriptor, bytes } => {
                let PropertyOwner::Message(message_id) = descriptor.owner else {
                    return Ok(());
                };
                if let Some(active) = self.active.get_mut(&(
                    message_id,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                )) {
                    active.hasher.update(bytes);
                    active.byte_len = active
                        .byte_len
                        .checked_add(
                            u64::try_from(bytes.len())
                                .map_err(|_| "placement property chunk is too large")?,
                        )
                        .ok_or("placement property length overflow")?;
                }
            }
            CatalogEvent::PropertyEnd(descriptor) => {
                let PropertyOwner::Message(message_id) = descriptor.owner else {
                    return Ok(());
                };
                if let Some(active) = self.active.remove(&(
                    message_id,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                )) {
                    self.properties
                        .entry(message_id)
                        .or_default()
                        .push(PropertyFingerprint {
                            id: active.id,
                            value_type: active.value_type,
                            byte_len: active.byte_len,
                            sha256: active.hasher.finalize().into(),
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
            _ => {}
        }
        Ok(())
    }
}

impl PlacementSink {
    fn finish(mut self) -> Result<Vec<PlacementFingerprint>, Box<dyn std::error::Error>> {
        if !self.active.is_empty() {
            return Err("placement property stream did not terminate".into());
        }
        let mut output = Vec::with_capacity(self.messages.len());
        for (message_id, (folder_id, associated, message_class)) in self.messages {
            let (parent_id, folder_name) = self
                .folders
                .get(&folder_id)
                .ok_or("placement message folder is missing")?;
            let parent_id = parent_id.ok_or("placement message folder has no parent")?;
            let parent = self
                .folders
                .get(&parent_id)
                .ok_or("placement parent folder is missing")?;
            let mut properties = self.properties.remove(&message_id).unwrap_or_default();
            properties.sort();
            output.push(PlacementFingerprint {
                folder_name: folder_name
                    .clone()
                    .ok_or("placement folder has no display name")?,
                folder_parent_is_store_root: parent.0.is_none(),
                associated,
                message_class,
                properties,
            });
        }
        output.sort();
        Ok(output)
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachmentNamedPropertyFingerprint {
    attachment_index: u32,
    identity: NamedPropertyIdentity,
    value_type: Option<u32>,
    byte_len: u64,
    sha256: [u8; 32],
}

#[derive(Default)]
struct AttachmentNamedPropertySink {
    pending: Option<(PropertyDescriptor, NamedPropertyIdentity)>,
    active: BTreeMap<(u32, u32, u32, u32), ActiveNamedProperty>,
    completed: Vec<AttachmentNamedPropertyFingerprint>,
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

impl CatalogSink for AttachmentNamedPropertySink {
    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
        match event {
            CatalogEvent::NamedProperty {
                descriptor,
                identity,
            } => {
                if self.pending.replace((descriptor, identity)).is_some() {
                    return Err("attachment named property identity was not consumed".to_owned());
                }
            }
            CatalogEvent::PropertyStart(descriptor) => {
                let Some((expected, identity)) = self.pending.take() else {
                    return Ok(());
                };
                if descriptor != expected {
                    return Err(
                        "attachment named property identity did not precede its value".to_owned(),
                    );
                }
                let PropertyOwner::Attachment { message_id, index } = descriptor.owner else {
                    return Ok(());
                };
                if self
                    .active
                    .insert(
                        (
                            message_id,
                            index,
                            descriptor.record_set_index,
                            descriptor.entry_index,
                        ),
                        ActiveNamedProperty {
                            identity,
                            value_type: descriptor.value_type,
                            byte_len: 0,
                            hasher: Sha256::new(),
                        },
                    )
                    .is_some()
                {
                    return Err("duplicate active attachment named property".to_owned());
                }
            }
            CatalogEvent::PropertyData { descriptor, bytes } => {
                let PropertyOwner::Attachment { message_id, index } = descriptor.owner else {
                    return Ok(());
                };
                if let Some(property) = self.active.get_mut(&(
                    message_id,
                    index,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                )) {
                    property.byte_len = property
                        .byte_len
                        .checked_add(u64::try_from(bytes.len()).map_err(|error| error.to_string())?)
                        .ok_or_else(|| "attachment named property size overflow".to_owned())?;
                    property.hasher.update(bytes);
                }
            }
            CatalogEvent::PropertyEnd(descriptor) => {
                let PropertyOwner::Attachment { message_id, index } = descriptor.owner else {
                    return Ok(());
                };
                if let Some(property) = self.active.remove(&(
                    message_id,
                    index,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                )) {
                    self.completed.push(AttachmentNamedPropertyFingerprint {
                        attachment_index: index,
                        identity: property.identity,
                        value_type: property.value_type,
                        byte_len: property.byte_len,
                        sha256: property.hasher.finalize().into(),
                    });
                }
            }
            CatalogEvent::PropertyAbort { descriptor, .. } => {
                let PropertyOwner::Attachment { message_id, index } = descriptor.owner else {
                    return Ok(());
                };
                self.active.remove(&(
                    message_id,
                    index,
                    descriptor.record_set_index,
                    descriptor.entry_index,
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MessageContentFingerprint {
    embedded_path: Vec<u32>,
    associated: bool,
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
    folder_identity: FolderIdentity,
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
    folder_identities: BTreeMap<u32, FolderIdentity>,
    sibling_name_counts: BTreeMap<(Option<u32>, String), u32>,
    unsupported_items: u64,
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
                let mut identity = match parent_id {
                    Some(parent) => self
                        .folder_identities
                        .get(&parent)
                        .cloned()
                        .ok_or_else(|| "folder identity preceded its parent".to_owned())?,
                    None => Vec::new(),
                };
                if parent_id.is_some()
                    && id != NID_IPM_SUBTREE
                    && let Some(name) = name
                {
                    let ordinal = self
                        .sibling_name_counts
                        .entry((parent_id, name.clone()))
                        .or_default();
                    identity.push((name.clone(), *ordinal));
                    *ordinal = ordinal
                        .checked_add(1)
                        .ok_or_else(|| "same-name sibling ordinal overflow".to_owned())?;
                    path.push(name);
                }
                if self.folder_paths.insert(id, path).is_some() {
                    return Err("duplicate folder identifier".to_owned());
                }
                if self.folder_identities.insert(id, identity).is_some() {
                    return Err("duplicate folder identity".to_owned());
                }
            }
            CatalogEvent::MessageStart {
                id,
                folder_id,
                parent_message_id,
                embedded_path,
                associated,
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
                    self.unsupported_items = self
                        .unsupported_items
                        .checked_add(1)
                        .ok_or_else(|| "unsupported item count overflow".to_owned())?;
                    return Ok(());
                }
                let (folder_path, folder_identity) = match (folder_id, parent_message_id) {
                    (Some(folder), _) => (
                        self.folder_paths
                            .get(&folder)
                            .cloned()
                            .ok_or_else(|| "message referenced an unknown folder".to_owned())?,
                        self.folder_identities
                            .get(&folder)
                            .cloned()
                            .ok_or_else(|| {
                                "message referenced an unknown folder identity".to_owned()
                            })?,
                    ),
                    (None, Some(parent)) => self
                        .active
                        .get(&parent)
                        .map(|message| {
                            (message.folder_path.clone(), message.folder_identity.clone())
                        })
                        .ok_or_else(|| {
                            "embedded message referenced an unknown parent".to_owned()
                        })?,
                    (None, None) => (Vec::new(), Vec::new()),
                };
                let message = MessageFingerprint {
                    folder_path,
                    folder_identity,
                    content: MessageContentFingerprint {
                        embedded_path,
                        associated,
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
            CatalogEvent::AttachmentEnd { message_id, index }
            | CatalogEvent::AttachmentAbort { message_id, index } => {
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
                        declared_size: if attachment.attachment_type == Some(i32::from(b'i')) {
                            None
                        } else {
                            attachment.declared_size
                        },
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
                        matches!(
                            id,
                            0x3001
                                | 0x3701
                                | 0x3702
                                | 0x3705
                                | 0x3708
                                | 0x3709
                                | 0x370a
                                | 0x370b
                                | 0x370d
                                | 0x370e
                                | 0x3712
                                | 0x3713
                                | 0x3714
                                | 0x7ffa..=0x7fff
                        )
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
    if property.id == 0x3701 {
        // Embedded-object data contains store-local node references. Attachment
        // payload bytes are compared independently; normalize this fingerprint
        // to presence and type so deterministic relocation is not a false diff.
        return PropertyFingerprint {
            id: property.id,
            value_type: property.value_type,
            byte_len: 0,
            sha256: Sha256::digest([]).into(),
        };
    }
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
struct RepairManifest {
    schema_version: u32,
    cases: Vec<RepairCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairCase {
    name: String,
    source_path: PathBuf,
    source_sha256: String,
    repaired_path: PathBuf,
    repaired_sha256: String,
    #[serde(default)]
    source_supplements: Vec<String>,
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
    let source = pstforge_core::SourceFile::open(path)?;
    independent_messages_with_issue_policy(&source, true, false).map(|(messages, _)| messages)
}

fn independent_generated_messages(
    path: &std::path::Path,
) -> Result<Vec<MessageFingerprint>, Box<dyn std::error::Error>> {
    let source = pstforge_core::SourceFile::open(path)?;
    independent_messages_with_issue_policy(&source, false, false).map(|(messages, _)| messages)
}

fn independent_repaired_messages(
    source: &pstforge_core::SourceFile,
) -> Result<RepairedMessageCatalog, Box<dyn std::error::Error>> {
    independent_messages_with_issue_policy(source, false, true)
}

fn independent_damaged_messages(
    source: &pstforge_core::SourceFile,
) -> Result<Vec<MessageFingerprint>, Box<dyn std::error::Error>> {
    let native = libpff_sys::PffFile::open_fd(source.file().as_fd())?;
    let mut sink = IndependentMessageSink::default();
    let catalog = native.catalog(&mut sink)?;
    if catalog.issues_dropped != 0
        || sink.unsupported_items != 0
        || !sink.active.is_empty()
        || !sink.attachments.is_empty()
        || !sink.properties.is_empty()
        || !sink.attachment_properties.is_empty()
    {
        return Err("damaged-source message catalog ended with unfinished state".into());
    }
    Ok(sink.completed)
}

fn independent_messages_with_issue_policy(
    source: &pstforge_core::SourceFile,
    allow_common_source_issues: bool,
    allow_repaired_reference_issues: bool,
) -> Result<RepairedMessageCatalog, Box<dyn std::error::Error>> {
    let native = libpff_sys::PffFile::open_fd(source.file().as_fd())?;
    let mut sink = IndependentMessageSink::default();
    let catalog = native.catalog(&mut sink)?;
    if catalog.issues.iter().any(|issue| {
        !(allow_common_source_issues
            && (matches!(
                (issue.operation, issue.message.as_str()),
                ("count attachments", message)
                    if message.contains("libpff_message_get_number_of_attachments")
            ) || matches!(
                (issue.operation, issue.message.as_str()),
                ("stream recipients", message)
                    if message.contains("libpff_message_get_recipients")
            ))
            || allow_repaired_reference_issues && is_repaired_reference_issue(issue))
    }) || catalog.issues_dropped != 0
        || (!allow_common_source_issues && sink.unsupported_items != 0)
        || !sink.active.is_empty()
        || !sink.attachments.is_empty()
        || !sink.properties.is_empty()
        || !sink.attachment_properties.is_empty()
    {
        return Err(format!(
            "independent message catalog was incomplete: issues={:?}, dropped={}, unsupported={}, active={}, attachments={}, properties={}, attachment_properties={}",
            catalog.issues,
            catalog.issues_dropped,
            sink.unsupported_items,
            sink.active.len(),
            sink.attachments.len(),
            sink.properties.len(),
            sink.attachment_properties.len(),
        )
        .into());
    }
    let unreadable_associated_folders = catalog
        .issues
        .iter()
        .filter(|issue| is_repaired_reference_issue(issue))
        .map(|issue| {
            issue
                .node_id
                .and_then(|folder_id| sink.folder_identities.get(&folder_id).cloned())
                .ok_or("repaired-reference issue has no readable folder path")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok((sink.completed, unreadable_associated_folders))
}

fn is_repaired_reference_issue(issue: &libpff_sys::CatalogIssue) -> bool {
    let Some(expected) = issue.node_id.and_then(repaired_reference_issue_message) else {
        return false;
    };
    issue.operation == "count folder associated contents" && issue.message == expected
}

fn repaired_reference_issue_message(node_id: u32) -> Option<String> {
    let descriptor_id = node_id.checked_add(13)?;
    Some(format!(
        "libpff get number of sub associated contents failed: libpff_local_descriptors_node_get_entry_data: invalid local descriptors node.\n\
libpff_local_descriptors_get_value_by_identifier: unable to retrieve node entry: 0 data.\n\
libpff_local_descriptors_tree_get_value_by_identifier: unable to retrieve index value: 10485855 from index.\n\
libpff_table_read_entry_value: unable to retrieve descriptor identifier: 10485855 from local descriptors.\n\
libpff_table_read_values_array: unable to read entry value: 0.\n\
libpff_table_read_7c_values: unable to read values array.\n\
libpff_table_read_values: unable to read 7c table values.\n\
libpff_table_read: unable to read table values.\n\
libpff_item_values_read: unable to read table.\n\
libpff_folder_determine_sub_associated_contents: unable to read descriptor identifier: {descriptor_id}.\n\
libpff_folder_get_number_of_sub_associated_contents: unable to determine sub associated contents."
    ))
}

#[test]
fn repaired_reference_issue_requires_the_exact_known_diagnostic() {
    let message = repaired_reference_issue_message(32_994).expect("test node id is bounded");
    let exact = libpff_sys::CatalogIssue {
        node_id: Some(32_994),
        operation: "count folder associated contents",
        message: message.clone(),
    };
    assert!(is_repaired_reference_issue(&exact));
    assert!(!is_repaired_reference_issue(&libpff_sys::CatalogIssue {
        message: format!("{message}\nadditional native failure"),
        ..exact.clone()
    }));
    assert!(!is_repaired_reference_issue(&libpff_sys::CatalogIssue {
        node_id: Some(32_995),
        ..exact
    }));
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

fn independent_attachment_named_properties(
    path: &std::path::Path,
) -> Result<Vec<AttachmentNamedPropertyFingerprint>, Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let native = libpff_sys::PffFile::open_fd(file.as_fd())?;
    let mut sink = AttachmentNamedPropertySink::default();
    let catalog = native.catalog(&mut sink)?;
    if !catalog.issues.is_empty()
        || catalog.issues_dropped != 0
        || sink.pending.is_some()
        || !sink.active.is_empty()
    {
        return Err("attachment named property catalog was incomplete".into());
    }
    sink.completed.sort_by_key(|property| {
        (
            property.attachment_index,
            property.identity.guid,
            format!("{:?}", property.identity.name),
        )
    });
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

fn independent_placement(
    path: &std::path::Path,
) -> Result<Vec<PlacementFingerprint>, Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let native = libpff_sys::PffFile::open_fd(file.as_fd())?;
    let mut sink = PlacementSink::default();
    let catalog = native.catalog(&mut sink)?;
    if !catalog.issues.is_empty()
        || catalog.issues_dropped != 0
        || catalog.unsupported_messages != 0
    {
        return Err("libpff placement catalog was incomplete".into());
    }
    sink.finish()
}

#[test]
fn root_and_associated_data_roundtrip_through_the_supervised_split()
-> Result<(), Box<dyn std::error::Error>> {
    use pstforge_pst::writer::{
        FidelityStore, MailFolderLocation, MailFolderRole, MailFolderSpec, MailStoreSpec,
        RawProperty, RawPropertyValue,
    };

    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source.pst");
    let mut normal = FidelityStore::default().message;
    normal.message_class = "IPM.Microsoft.SniffData".to_owned();
    normal.subject = "root placement checkpoint".to_owned();
    normal.sender_name.clear();
    normal.sender_email.clear();
    normal.recipients.clear();
    normal.attachments.clear();
    normal.body_text = None;
    normal.body_html = None;
    normal.body_rtf = None;
    normal.native_body = None;
    normal.rtf_in_sync = false;
    normal.internet_headers = None;
    normal.named_properties.clear();
    normal.spooled_properties.clear();
    normal.unsupported_properties.clear();
    normal.raw_properties = vec![
        RawProperty {
            id: 0x6000,
            value: RawPropertyValue::Integer32(42),
        },
        RawProperty {
            id: 0x6001,
            value: RawPropertyValue::Unicode("normal root value".to_owned()),
        },
        RawProperty {
            id: 0x6002,
            value: RawPropertyValue::Binary(vec![0, 1, 2, 3, 0xFE, 0xFF]),
        },
    ];
    let mut associated = normal.clone();
    associated.message_class = "IPM.Configuration.PSTForge".to_owned();
    associated.subject = "subject fallback must not replace display name".to_owned();
    associated.raw_properties[0].value = RawPropertyValue::Integer32(84);
    associated.raw_properties[1].value =
        RawPropertyValue::Unicode("hidden associated value".to_owned());
    associated.raw_properties.push(RawProperty {
        id: 0x3001,
        value: RawPropertyValue::Unicode("associated placement checkpoint".to_owned()),
    });
    let fixture = MailStoreSpec {
        store_name: "PSTForge placement source".to_owned(),
        record_key: *b"PSTForgePlace001",
        folders: vec![
            MailFolderSpec {
                path: vec!["Freebusy Data".to_owned()],
                location: MailFolderLocation::StoreRoot,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: vec![normal],
                associated_messages: Vec::new(),
            },
            MailFolderSpec {
                path: vec!["IPM_COMMON_VIEWS".to_owned()],
                location: MailFolderLocation::StoreRoot,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: Vec::new(),
                associated_messages: vec![associated],
            },
        ],
    };
    pstforge_pst::writer::create_mail_store(&source, &fixture)?;
    let source_identity = pstforge_core::SourceFile::open(&source)?.identity().clone();
    let expected = independent_placement(&source)?;
    if expected.len() != 2
        || expected
            .iter()
            .any(|item| !item.folder_parent_is_store_root)
        || expected.iter().filter(|item| item.associated).count() != 1
        || expected
            .iter()
            .any(|item| item.properties.len() != if item.associated { 5 } else { 4 })
    {
        return Err("source placement fixture does not meet its contract".into());
    }

    let recovery_verify = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("verify")
        .arg(&source)
        .arg("--mode")
        .arg("recovery")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !recovery_verify.status.success() {
        return Err(format!(
            "supervised recovery verification failed: {}",
            String::from_utf8_lossy(&recovery_verify.stderr)
        )
        .into());
    }
    let recovery_verify: serde_json::Value = serde_json::from_slice(&recovery_verify.stdout)?;
    if recovery_verify["mode"] != "recovery"
        || recovery_verify["inventory"]["recovered_items"].as_u64() != Some(0)
        || recovery_verify["inventory"]["orphan_items"].as_u64() != Some(0)
        || recovery_verify["source_unchanged"].as_bool() != Some(true)
    {
        return Err("supervised recovery verification returned the wrong scope".into());
    }

    let contained_crash = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("verify")
        .arg(&source)
        .arg("--mode")
        .arg("recovery")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .env("PSTFORGE_TEST_VERIFY_WORKER_ABORT", "1")
        .output()?;
    if contained_crash.status.code() != Some(3) || !contained_crash.stdout.is_empty() {
        return Err("recovery verification did not contain its native worker abort".into());
    }

    let mut killed_supervisor = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("verify")
        .arg(&source)
        .arg("--mode")
        .arg("recovery")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .env("PSTFORGE_TEST_VERIFY_WORKER_STALL", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let supervisor_id = killed_supervisor.id();
    let child_deadline = Instant::now() + Duration::from_secs(10);
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
        if Instant::now() >= child_deadline {
            let _ = killed_supervisor.kill();
            return Err("verification worker did not start before signal deadline".into());
        }
        thread::sleep(Duration::from_millis(25));
    };
    killed_supervisor.kill()?;
    let status = killed_supervisor.wait()?;
    if status.success() {
        return Err("killed verification supervisor unexpectedly succeeded".into());
    }
    let worker_deadline = Instant::now() + Duration::from_secs(5);
    while PathBuf::from(format!("/proc/{worker_id}")).exists() {
        if Instant::now() >= worker_deadline {
            return Err("verification worker outlived its killed supervisor".into());
        }
        thread::sleep(Duration::from_millis(25));
    }

    let output = directory.path().join("split");
    let result = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&source)
        .arg("--output")
        .arg(&output)
        .arg("--restartable")
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !result.status.success() {
        return Err(format!(
            "placement split failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    if report["partial"].as_bool() != Some(false)
        || report["parts"].as_array().map(Vec::len) != Some(1)
    {
        return Err("placement split was not a complete one-part result".into());
    }
    let regenerated = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("report")
        .arg(&output)
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !regenerated.status.success() {
        return Err(format!(
            "report regeneration failed: {}",
            String::from_utf8_lossy(&regenerated.stderr)
        )
        .into());
    }
    let regenerated: serde_json::Value = serde_json::from_slice(&regenerated.stdout)?;
    if regenerated["schema_version"] != "1.0.0"
        || regenerated["command"] != "report"
        || regenerated["split"] != report
    {
        return Err("regenerated JSON does not match the finalized split report".into());
    }
    let generated = output.join("parts/part-0001.pst");
    let actual = independent_placement(&generated)?;
    if actual != expected {
        return Err(format!(
            "root hierarchy, associated placement, class, or property changed: expected {expected:?}, actual {actual:?}"
        )
        .into());
    }
    let pffinfo = Command::new("pffinfo").arg(&generated).output()?;
    if !pffinfo.status.success() {
        return Err("pffinfo rejected the placement output".into());
    }
    pstforge_core::SourceFile::open(&source)?
        .verify_unchanged()
        .map_err(|_| "placement source changed during split")?;
    if pstforge_core::SourceFile::open(&source)?.identity() != &source_identity {
        return Err("placement source identity changed during split".into());
    }
    let mut damaged_output = fs::read(&generated)?;
    let changed = damaged_output
        .get_mut(1024)
        .ok_or("generated placement PST is unexpectedly short")?;
    *changed ^= 1;
    fs::write(&generated, damaged_output)?;
    let rejected = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("report")
        .arg(&output)
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if rejected.status.code() != Some(4) || !rejected.stdout.is_empty() {
        return Err("report did not reject changed finalized output with status 4".into());
    }
    let direct_output = directory.path().join("direct-split");
    let direct = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("split")
        .arg(&source)
        .arg("--output")
        .arg(&direct_output)
        .arg("--max-pst-size")
        .arg("4GiB")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !direct.status.success() {
        return Err(format!(
            "direct placement split failed: {}",
            String::from_utf8_lossy(&direct.stderr)
        )
        .into());
    }
    let direct: serde_json::Value = serde_json::from_slice(&direct.stdout)?;
    if !direct["parts"][0]["sha256"].is_null() {
        return Err("direct split unexpectedly calculated a finalized-part hash".into());
    }
    let direct_report = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("report")
        .arg(&direct_output)
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if !direct_report.status.success()
        || serde_json::from_slice::<serde_json::Value>(&direct_report.stdout)?["split"] != direct
    {
        return Err("direct report did not preserve the hash-free split result".into());
    }
    let direct_part = direct_output.join("parts/part-0001.pst");
    let mut replaced = fs::read(&direct_part)?;
    let changed = replaced
        .get_mut(1024)
        .ok_or("direct placement PST is unexpectedly short")?;
    *changed ^= 1;
    let replacement = direct_output.join("parts/replacement.pst");
    fs::write(&replacement, replaced)?;
    fs::set_permissions(&replacement, fs::metadata(&direct_part)?.permissions())?;
    fs::rename(&replacement, &direct_part)?;
    let rejected_replacement = Command::new(env!("CARGO_BIN_EXE_pstforge"))
        .arg("report")
        .arg(&direct_output)
        .arg("--json")
        .arg("--color")
        .arg("never")
        .output()?;
    if rejected_replacement.status.code() != Some(4) || !rejected_replacement.stdout.is_empty() {
        return Err("report accepted a same-length replacement for a direct part".into());
    }
    Ok(())
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

fn verify_repair_message_fidelity(
    expected: &[MessageFingerprint],
    actual: Vec<MessageFingerprint>,
) -> Result<Vec<MessageFingerprint>, RepairMismatch> {
    let mut unmatched = actual;
    for reference in expected {
        let Some(position) = unmatched.iter().position(|generated| {
            generated.folder_identity == reference.folder_identity
                && content_matches_current_recovery_policy(&reference.content, &generated.content)
        }) else {
            let categories = unmatched
                .iter()
                .find(|generated| generated.folder_identity == reference.folder_identity)
                .map(|generated| fingerprint_difference(&reference.content, &generated.content))
                .filter(|categories| !categories.is_empty())
                .unwrap_or_else(|| vec!["placement or multiplicity"]);
            return Err(RepairMismatch {
                categories: categories.into_iter().collect(),
            });
        };
        unmatched.swap_remove(position);
    }
    Ok(unmatched)
}

#[derive(Debug)]
struct RepairMismatch {
    categories: BTreeSet<&'static str>,
}

impl std::fmt::Display for RepairMismatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let categories = self
            .categories
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            formatter,
            "semantic message multiset differs from repaired reference in: {categories}"
        )
    }
}

impl std::error::Error for RepairMismatch {}

fn verify_source_supplemented_fidelity(
    references: &[MessageFingerprint],
    sources: &[MessageFingerprint],
    actual: Vec<MessageFingerprint>,
    supplements: &BTreeSet<&str>,
) -> Result<(Vec<MessageFingerprint>, Vec<bool>), Box<dyn std::error::Error>> {
    let mut consumed_sources = vec![false; sources.len()];
    let mut unmatched_actual = actual;
    for reference in references {
        let Some(source_position) = sources.iter().enumerate().position(|(index, source)| {
            !consumed_sources[index]
                && source.complete
                && source.folder_identity == reference.folder_identity
                && content_matches_except_supplements(
                    &reference.content,
                    &source.content,
                    supplements,
                )
        }) else {
            return Err("source supplement has no matching repaired-reference item".into());
        };
        consumed_sources[source_position] = true;
        let source = &sources[source_position];
        let expected = supplemented_content(&reference.content, &source.content, supplements)?;
        let Some(actual_position) = unmatched_actual.iter().position(|generated| {
            generated.folder_identity == reference.folder_identity && generated.content == expected
        }) else {
            return Err("output does not match the repaired/source hybrid reference".into());
        };
        unmatched_actual.swap_remove(actual_position);
    }
    Ok((unmatched_actual, consumed_sources))
}

fn content_matches_except_supplements(
    reference: &MessageContentFingerprint,
    source: &MessageContentFingerprint,
    supplements: &BTreeSet<&str>,
) -> bool {
    let mut reference = reference.clone();
    let mut source = source.clone();
    normalize_current_recovery_policy(&mut reference);
    normalize_current_recovery_policy(&mut source);
    if supplements.contains("recipients") {
        reference.recipients.clear();
        source.recipients.clear();
    }
    if supplements.contains("body properties") {
        reference.body_properties.clear();
        source.body_properties.clear();
    }
    reference == source
}

fn supplemented_content(
    reference: &MessageContentFingerprint,
    source: &MessageContentFingerprint,
    supplements: &BTreeSet<&str>,
) -> Result<MessageContentFingerprint, Box<dyn std::error::Error>> {
    let mut expected = reference.clone();
    let mut source = source.clone();
    normalize_current_recovery_policy(&mut expected);
    normalize_current_recovery_policy(&mut source);
    for supplement in supplements {
        match *supplement {
            "recipients" => {
                require_multiset_subset(&expected.recipients, &source.recipients)
                    .map_err(|_| "source supplement replaces a repaired recipient")?;
                expected.recipients = source.recipients.clone();
            }
            "body properties" => {
                require_multiset_subset(&expected.body_properties, &source.body_properties)
                    .map_err(|_| "source supplement replaces a repaired body property")?;
                expected.body_properties = source.body_properties.clone();
            }
            _ => return Err("repair manifest names an unsupported source supplement".into()),
        }
    }
    Ok(expected)
}

fn require_multiset_subset<T: Eq>(subset: &[T], superset: &[T]) -> Result<(), ()> {
    let mut unmatched = superset.iter().collect::<Vec<_>>();
    for expected in subset {
        let Some(position) = unmatched
            .iter()
            .position(|candidate| *candidate == expected)
        else {
            return Err(());
        };
        unmatched.swap_remove(position);
    }
    Ok(())
}

#[test]
fn repair_supplement_accepts_only_multiset_additions() {
    assert!(require_multiset_subset(&[1, 1, 2], &[2, 1, 3, 1]).is_ok());
    assert!(require_multiset_subset(&[1, 1, 2], &[1, 2, 3]).is_err());
    assert!(require_multiset_subset(&[1, 2], &[1, 3, 4]).is_err());
}

#[test]
fn repair_extra_cannot_reuse_a_consumed_source_occurrence() {
    let message = MessageFingerprint {
        folder_path: vec!["Inbox".to_owned()],
        folder_identity: vec![("Inbox".to_owned(), 0)],
        content: MessageContentFingerprint {
            embedded_path: Vec::new(),
            associated: true,
            message_class: Some("IPM.Note".to_owned()),
            subject: Some("associated".to_owned()),
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            recipients: Vec::new(),
            attachments: Vec::new(),
            body_properties: Vec::new(),
        },
        complete: true,
    };
    assert!(
        verify_extra_messages_from_source(
            std::slice::from_ref(&message),
            std::slice::from_ref(&message),
            std::slice::from_ref(&message),
            None,
        )
        .is_err()
    );
    assert!(
        verify_extra_messages_from_source(
            &[],
            std::slice::from_ref(&message),
            std::slice::from_ref(&message),
            Some(vec![true]),
        )
        .is_err()
    );
}

fn verify_extra_messages_from_source(
    references: &[MessageFingerprint],
    extras: &[MessageFingerprint],
    sources: &[MessageFingerprint],
    consumed_sources: Option<Vec<bool>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut consumed_sources = match consumed_sources {
        Some(consumed) if consumed.len() == sources.len() => consumed,
        Some(_) => {
            return Err("source-consumption state length differs from source catalog".into());
        }
        None => vec![false; sources.len()],
    };
    if !consumed_sources.iter().any(|consumed| *consumed) {
        for reference in references {
            if let Some(position) = sources.iter().enumerate().position(|(index, source)| {
                !consumed_sources[index]
                    && source.complete
                    && source.folder_identity == reference.folder_identity
                    && content_matches_current_recovery_policy(&source.content, &reference.content)
            }) {
                consumed_sources[position] = true;
            }
        }
    }
    for extra in extras {
        let Some(position) = sources.iter().enumerate().position(|(index, source)| {
            !consumed_sources[index]
                && source.complete
                && source.folder_identity == extra.folder_identity
                && content_matches_current_recovery_policy(&source.content, &extra.content)
        }) else {
            return Err("extra associated item has no exact readable source match".into());
        };
        consumed_sources[position] = true;
    }
    Ok(())
}

fn finish_repair_case<T>(
    case_name: &str,
    directory: tempfile::TempDir,
    case_result: Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    let cleanup_result = directory.close();
    match (case_result, cleanup_result) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => {
            Err(format!("{case_name} scratch cleanup failed: {cleanup}").into())
        }
        (Err(error), Err(cleanup)) => Err(format!(
            "{case_name} failed: {error}; scratch cleanup also failed: {cleanup}"
        )
        .into()),
    }
}

fn finish_repair_immutability<T>(
    case_name: &str,
    case_result: Result<T, Box<dyn std::error::Error>>,
    immutability_result: Result<(), Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    match (case_result, immutability_result) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(immutability)) => Err(immutability),
        (Err(error), Err(immutability)) => Err(format!(
            "{case_name} failed: {error}; immutability verification also failed: {immutability}"
        )
        .into()),
    }
}

fn verify_repair_pair_unchanged(
    case_name: &str,
    source: &pstforge_core::SourceFile,
    source_before: &pstforge_core::SourceIdentity,
    repaired: &pstforge_core::SourceFile,
    repaired_before: &pstforge_core::SourceIdentity,
) -> Result<(), Box<dyn std::error::Error>> {
    let source_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        source.verify_unchanged()?;
        if source.identity() != source_before {
            return Err(format!("{case_name} source identity changed").into());
        }
        Ok(())
    })();
    let repaired_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        repaired.verify_unchanged()?;
        if repaired.identity() != repaired_before {
            return Err(format!("{case_name} repaired-reference identity changed").into());
        }
        Ok(())
    })();
    match (source_result, repaired_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(source), Ok(())) => Err(source),
        (Ok(()), Err(repaired)) => Err(repaired),
        (Err(source), Err(repaired)) => Err(format!(
            "{case_name} source verification failed: {source}; repaired-reference verification also failed: {repaired}"
        )
        .into()),
    }
}

#[test]
fn failed_repair_case_removes_sensitive_scratch() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().to_path_buf();
    fs::write(path.join("generated.pst"), b"private mailbox bytes")?;
    let result: Result<(), Box<dyn std::error::Error>> =
        Err("forced post-write validation failure".into());
    assert!(finish_repair_case("failure-cleanup", directory, result).is_err());
    assert!(!path.exists());
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
        if !unmatched.iter().any(|source| {
            content_matches_current_recovery_policy(&source.content, &generated.content)
        }) {
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
            content_matches_current_recovery_policy(&source.content, &generated.content)
                && generated.folder_path == source.folder_path
        }) else {
            let source_depth = unmatched
                .iter()
                .find(|source| {
                    content_matches_current_recovery_policy(&source.content, &generated.content)
                })
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

fn content_matches_current_recovery_policy(
    source: &MessageContentFingerprint,
    generated: &MessageContentFingerprint,
) -> bool {
    let mut expected = source.clone();
    normalize_current_recovery_policy(&mut expected);
    expected == *generated
}

fn normalize_current_recovery_policy(expected: &mut MessageContentFingerprint) {
    if expected.message_class.is_none() {
        expected.message_class = Some("IPM.Note".to_owned());
    }
    if expected.subject.as_deref().is_none_or(str::is_empty) {
        expected.subject = None;
    }
    let sender_name = expected
        .sender_name
        .clone()
        .filter(|value| !value.is_empty());
    let sender_email = expected
        .sender_email
        .clone()
        .filter(|value| !value.is_empty());
    match (sender_name, sender_email) {
        (None, None) => {
            expected.sender_name = None;
            expected.sender_email = None;
        }
        (None, Some(address)) => {
            expected.sender_name = Some(address.clone());
            expected.sender_email = Some(address);
        }
        (Some(name), None) => {
            expected.sender_name = Some(name.clone());
            expected.sender_email = Some(name);
        }
        (Some(name), Some(address)) => {
            expected.sender_name = Some(name);
            expected.sender_email = Some(address);
        }
    }
    let source_delivery = expected.delivery_filetime;
    if expected.submit_filetime.is_none() {
        expected.submit_filetime = Some(0);
    }
    if expected.delivery_filetime.is_none() {
        expected.delivery_filetime = Some(0);
    }
    add_i32_fallback(&mut expected.body_properties, 0x0E07, 1);
    add_i32_fallback(&mut expected.body_properties, 0x3FDE, 65_001);
    add_i64_fallback(
        &mut expected.body_properties,
        0x3007,
        source_delivery.unwrap_or_default(),
    );
    add_i64_fallback(
        &mut expected.body_properties,
        0x3008,
        source_delivery.unwrap_or_default(),
    );
    if expected.associated
        && !expected
            .body_properties
            .iter()
            .any(|property| property.id == 0x3001)
    {
        let display_name = expected
            .subject
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("(no subject)");
        let bytes = display_name
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        expected
            .body_properties
            .push(property_fingerprint(0x3001, 0x001F, &bytes));
    }
    expected.body_properties.sort();
}

#[test]
fn recovery_policy_normalizes_explicit_empty_visible_metadata() {
    let mut expected = MessageContentFingerprint {
        embedded_path: Vec::new(),
        associated: false,
        message_class: Some("IPM.Contact".to_owned()),
        subject: Some(String::new()),
        sender_name: Some(String::new()),
        sender_email: Some("contact@example.com".to_owned()),
        submit_filetime: Some(1),
        delivery_filetime: Some(2),
        recipients: Vec::new(),
        attachments: Vec::new(),
        body_properties: Vec::new(),
    };
    normalize_current_recovery_policy(&mut expected);
    assert_eq!(expected.subject, None);
    assert_eq!(expected.sender_name.as_deref(), Some("contact@example.com"));
    assert_eq!(
        expected.sender_email.as_deref(),
        Some("contact@example.com")
    );
}

fn add_i32_fallback(properties: &mut Vec<PropertyFingerprint>, id: u32, value: i32) {
    if properties.iter().any(|property| property.id == id) {
        return;
    }
    properties.push(property_fingerprint(id, 0x0003, &value.to_le_bytes()));
}

fn add_i64_fallback(properties: &mut Vec<PropertyFingerprint>, id: u32, value: u64) {
    if properties.iter().any(|property| property.id == id) {
        return;
    }
    properties.push(property_fingerprint(id, 0x0040, &value.to_le_bytes()));
}

fn property_fingerprint(id: u32, value_type: u32, bytes: &[u8]) -> PropertyFingerprint {
    PropertyFingerprint {
        id,
        value_type: Some(value_type),
        byte_len: u64::try_from(bytes.len()).unwrap_or_default(),
        sha256: Sha256::digest(bytes).into(),
    }
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
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
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
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
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
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
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
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
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
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
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
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
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
#[ignore = "requires the external v042-distribution-list-source corpus case"]
fn milestone_0_4_2_distribution_list_roundtrip_through_libpff()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-distribution-list-source")
        .ok_or("manifest has no v042-distribution-list-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
        return Err("distribution-list source SHA-256 does not match its manifest".into());
    }

    let source_messages = independent_messages(&case.path)?;
    if source_messages.len() != 1
        || source_messages[0].folder_path != ["Contacts"]
        || source_messages[0].content.message_class.as_deref() != Some("IPM.DistList")
        || source_messages[0].content.subject.as_deref()
            != Some("PSTForge distribution list checkpoint")
        || source_messages[0].content.sender_name.is_some()
        || source_messages[0].content.sender_email.is_some()
        || !source_messages[0].content.recipients.is_empty()
        || !source_messages[0].complete
    {
        return Err("distribution-list source does not match the item contract".into());
    }
    let source_named = independent_named_properties(&case.path)?;
    let expected = [(0x8053, 0x001F), (0x8054, 0x1102), (0x8055, 0x1102)];
    if source_named.len() != expected.len()
        || !expected.iter().all(|(lid, property_type)| {
            source_named.iter().any(|property| {
                property.owner.message_class.as_deref() == Some("IPM.DistList")
                    && property.owner.subject.as_deref()
                        == Some("PSTForge distribution list checkpoint")
                    && property.identity.guid
                        == [
                            0x04, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00,
                            0x00, 0x00, 0x00, 0x46,
                        ]
                    && property.identity.name == NamedPropertyName::Numeric(*lid)
                    && property.value_type == Some(*property_type)
            })
        })
    {
        return Err(
            format!("distribution-list named-property contract changed: {source_named:?}").into(),
        );
    }
    if independent_folder_classes(&case.path)?
        .get(&vec!["Contacts".to_owned()])
        .map(String::as_str)
        != Some("IPF.Contact")
    {
        return Err("distribution-list source folder is not IPF.Contact".into());
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
            "distribution-list split failed: {}",
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
        return Err("distribution-list split did not report complete preservation".into());
    }
    let generated = job.join("parts/part-0001.pst");
    verify_exact_message_fidelity(source_messages, independent_messages(&generated)?)?;
    if independent_named_properties(&generated)? != source_named {
        return Err("distribution-list named-property identity or payload changed".into());
    }
    if independent_folder_classes(&generated)?
        .get(&vec!["Contacts".to_owned()])
        .map(String::as_str)
        != Some("IPF.Contact")
    {
        return Err("generated distribution-list folder is not IPF.Contact".into());
    }
    if pstforge_core::SourceFile::open(&case.path)?.identity() != &identity {
        return Err("distribution-list source identity changed during the split".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the external v042-document-source corpus case"]
fn milestone_0_4_2_document_object_roundtrip_through_libpff()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-document-source")
        .ok_or("manifest has no v042-document-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
        return Err("Document source SHA-256 does not match its manifest".into());
    }

    let source_messages = independent_messages(&case.path)?;
    let source = source_messages
        .first()
        .ok_or("Document source has no message")?;
    if source_messages.len() != 1
        || source.folder_path != ["Documents"]
        || source.content.message_class.as_deref() != Some("IPM.Document.Word.Document.12")
        || source.content.subject.as_deref() != Some("PSTForge document checkpoint.docx")
        || source.content.sender_name.is_some()
        || source.content.sender_email.is_some()
        || !source.content.recipients.is_empty()
        || source.content.attachments.len() != 1
        || source.content.attachments[0].attachment_type != Some(100)
        || source.content.attachments[0].filename.as_deref()
            != Some("PSTForge document checkpoint.docx")
        || source.content.attachments[0].streamed_size == 0
        || lower_hex(&source.content.attachments[0].sha256)
            != "6189ada04b0f10ed91272485315c5d4d5b90e8a6589fabc145a5b33af8181b33"
        || !source.complete
    {
        return Err(format!("Document source does not match the item contract: {source:?}").into());
    }

    let source_named = independent_named_properties(&case.path)?;
    let expected = [
        ("Comments", 0x001F),
        ("Keywords", 0x101F),
        ("PageCount", 0x0003),
        ("Title", 0x001F),
    ];
    let public_strings = [
        0x29, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    if source_named.len() != expected.len()
        || !expected.iter().all(|(name, property_type)| {
            source_named.iter().any(|property| {
                property.owner.message_class.as_deref() == Some("IPM.Document.Word.Document.12")
                    && property.owner.subject.as_deref()
                        == Some("PSTForge document checkpoint.docx")
                    && property.identity.guid == public_strings
                    && property.identity.name == NamedPropertyName::String((*name).to_owned())
                    && property.value_type == Some(*property_type)
                    && property.byte_len > 0
            })
        })
    {
        return Err(format!("Document named-property contract changed: {source_named:?}").into());
    }
    if independent_folder_classes(&case.path)?
        .get(&vec!["Documents".to_owned()])
        .map(String::as_str)
        != Some("IPF.Note")
    {
        return Err("Document source folder is not IPF.Note".into());
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
            "Document split failed: {}",
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
        return Err("Document split did not report complete preservation".into());
    }
    let generated = job.join("parts/part-0001.pst");
    verify_exact_message_fidelity(source_messages, independent_messages(&generated)?)?;
    if independent_named_properties(&generated)? != source_named {
        return Err("Document named-property identity or payload changed".into());
    }
    if independent_folder_classes(&generated)?
        .get(&vec!["Documents".to_owned()])
        .map(String::as_str)
        != Some("IPF.Note")
    {
        return Err("generated Document folder is not IPF.Note".into());
    }
    if pstforge_core::SourceFile::open(&case.path)?.identity() != &identity {
        return Err("Document source identity changed during the split".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the external v042-reference-attachments-source corpus case"]
fn milestone_0_4_2_reference_attachments_roundtrip_through_libpff()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-reference-attachments-source")
        .ok_or("manifest has no v042-reference-attachments-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
        return Err("reference attachment source SHA-256 does not match its manifest".into());
    }

    let source_messages = independent_messages(&case.path)?;
    let source = source_messages
        .first()
        .ok_or("reference attachment source has no message")?;
    let methods = [2_i32, 3, 4, 7];
    let relationships = [
        (
            "shared-reference.txt",
            r"\\unreachable.invalid\recovery\shared-reference.txt",
            Some("shared-reference.txt"),
        ),
        (
            "resolved-reference.txt",
            r"\\unreachable.invalid\recovery\resolved-reference.txt",
            Some("resolved-reference.txt"),
        ),
        (
            "reference-only.txt",
            r"Z:\unavailable\reference-only.txt",
            Some("reference-only.txt"),
        ),
        (
            "web-reference.docx",
            "https://example.invalid/recovery/web-reference.docx",
            None,
        ),
    ];
    let unicode_hash = |value: &str| -> [u8; 32] {
        let bytes = value
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        Sha256::digest(bytes).into()
    };
    if source_messages.len() != 1
        || source.folder_path != ["Reference Attachments"]
        || source.content.subject.as_deref() != Some("Reference attachment fidelity checkpoint")
        || source.content.attachments.len() != 4
        || !source.complete
        || source
            .content
            .attachments
            .iter()
            .zip(methods.into_iter().zip(relationships))
            .any(
                |(attachment, (method, (filename, long_pathname, pathname)))| {
                    let expected_method_hash: [u8; 32] =
                        Sha256::digest(method.to_le_bytes()).into();
                    attachment.attachment_type != Some(i32::from(b'r'))
                        || attachment.filename.as_deref() != Some(filename)
                        || attachment.declared_size.is_some()
                        || attachment.streamed_size != 0
                        || attachment
                            .rendering_properties
                            .iter()
                            .any(|property| property.id == 0x3701)
                        || !attachment.rendering_properties.iter().any(|property| {
                            property.id == 0x3705
                                && property.value_type == Some(0x0003)
                                && property.sha256 == expected_method_hash
                        })
                        || !attachment.rendering_properties.iter().any(|property| {
                            property.id == 0x370D
                                && property.value_type == Some(0x001F)
                                && property.sha256 == unicode_hash(long_pathname)
                        })
                        || match pathname {
                            Some(pathname) => {
                                !attachment.rendering_properties.iter().any(|property| {
                                    property.id == 0x3708
                                        && property.value_type == Some(0x001F)
                                        && property.sha256 == unicode_hash(pathname)
                                })
                            }
                            None => attachment
                                .rendering_properties
                                .iter()
                                .any(|property| property.id == 0x3708),
                        }
                },
            )
    {
        return Err(format!(
            "reference attachment source does not match the relationship contract: {source:?}"
        )
        .into());
    }

    let source_named = independent_attachment_named_properties(&case.path)?;
    let attachment_set = [
        0x7F, 0x7F, 0x35, 0x96, 0xE1, 0x59, 0xD0, 0x47, 0x99, 0xA7, 0x46, 0x51, 0x5C, 0x18, 0x3B,
        0x54,
    ];
    let expected_names: [(&str, u32, [u8; 32]); 3] = [
        (
            "AttachmentOriginalPermissionType",
            0x0003,
            Sha256::digest(1_i32.to_le_bytes()).into(),
        ),
        (
            "AttachmentPermissionType",
            0x0003,
            Sha256::digest(2_i32.to_le_bytes()).into(),
        ),
        (
            "AttachmentProviderType",
            0x001F,
            unicode_hash("RecoveryProvider"),
        ),
    ];
    if source_named.len() != expected_names.len()
        || !expected_names
            .iter()
            .all(|(name, property_type, expected_hash)| {
                source_named.iter().any(|property| {
                    property.attachment_index == 3
                        && property.identity.guid == attachment_set
                        && property.identity.name == NamedPropertyName::String((*name).to_owned())
                        && property.value_type == Some(*property_type)
                        && property.sha256 == *expected_hash
                })
            })
    {
        return Err(
            format!("reference attachment NAMEID contract changed: {source_named:?}").into(),
        );
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
            "reference attachment split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if report["partial"].as_bool() != Some(false)
        || report["written_candidates"].as_u64() != Some(1)
        || report["parts"].as_array().map(Vec::len) != Some(1)
        || report["parts"][0]["omitted_properties"].as_u64() != Some(0)
        || report["parts"][0]["omitted_attachments"].as_u64() != Some(0)
    {
        return Err("reference attachment split did not report complete preservation".into());
    }
    let generated = job.join("parts/part-0001.pst");
    let generated_messages = independent_messages(&generated)?;
    verify_exact_message_fidelity(source_messages, generated_messages)?;
    if independent_attachment_named_properties(&generated)? != source_named {
        return Err("reference attachment NAMEID identity or payload changed".into());
    }
    if pstforge_core::SourceFile::open(&case.path)?.identity() != &identity {
        return Err("reference attachment source identity changed during the split".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the external v042-ole-attachments-source corpus case"]
fn milestone_0_4_2_ole_attachments_roundtrip_through_libpff()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-ole-attachments-source")
        .ok_or("manifest has no v042-ole-attachments-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
        return Err("OLE attachment source SHA-256 does not match its manifest".into());
    }

    let source_messages = independent_messages(&case.path)?;
    let source = source_messages
        .first()
        .ok_or("OLE attachment source has no message")?;
    let method_hash: [u8; 32] = Sha256::digest(6_i32.to_le_bytes()).into();
    if source_messages.len() != 1
        || source.content.subject.as_deref() != Some("OLE attachment fidelity checkpoint")
        || source.content.attachments.len() != 2
        || !source.complete
        || source
            .content
            .attachments
            .iter()
            .enumerate()
            .any(|(index, attachment)| {
                let expected_type = if index == 0 { 0x000D } else { 0x0102 };
                attachment.streamed_size == 0
                    || attachment.declared_size != Some(attachment.streamed_size)
                    || !attachment.rendering_properties.iter().any(|property| {
                        property.id == 0x3701 && property.value_type == Some(expected_type)
                    })
                    || !attachment.rendering_properties.iter().any(|property| {
                        property.id == 0x3705
                            && property.value_type == Some(0x0003)
                            && property.sha256 == method_hash
                    })
            })
    {
        return Err(format!(
            "OLE attachment source does not match the method/data contract: {source:?}"
        )
        .into());
    }
    let first = &source.content.attachments[0];
    if ![0x3702, 0x3709, 0x370A].into_iter().all(|id| {
        first
            .rendering_properties
            .iter()
            .any(|property| property.id == id && property.value_type == Some(0x0102))
    }) {
        return Err(
            format!("OLE2 attachment source is missing optional metadata: {first:?}").into(),
        );
    }
    let second = &source.content.attachments[1];
    if !second.rendering_properties.iter().any(|property| {
        property.id == 0x3709 && property.value_type == Some(0x0102) && property.byte_len == 0
    }) {
        return Err(format!(
            "OLE1 attachment source lost the explicitly empty rendering property: {second:?}"
        )
        .into());
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
    if output.stdout.is_empty() {
        return Err(format!(
            "OLE attachment split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let generated = job.join("parts/part-0001.pst");
    let generated_messages = independent_messages(&generated)?;
    verify_exact_message_fidelity(source_messages, generated_messages)?;
    if report["partial"].as_bool() != Some(false)
        || !output.status.success()
        || report["written_candidates"].as_u64() != Some(1)
        || report["parts"].as_array().map(Vec::len) != Some(1)
        || report["parts"][0]["omitted_properties"].as_u64() != Some(0)
        || report["parts"][0]["omitted_attachments"].as_u64() != Some(0)
    {
        return Err(
            format!("OLE attachment split did not report complete preservation: {report}").into(),
        );
    }
    if pstforge_core::SourceFile::open(&case.path)?.identity() != &identity {
        return Err("OLE attachment source identity changed during the split".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the external v042-outlook-ole-object-source corpus case"]
fn milestone_0_4_2_outlook_ole_objects_roundtrip_through_libpff()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-outlook-ole-object-source")
        .ok_or("manifest has no v042-outlook-ole-object-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
        return Err("Outlook OLE source SHA-256 does not match its manifest".into());
    }

    let source_messages = independent_messages(&case.path)?;
    let source = source_messages
        .first()
        .ok_or("Outlook OLE source has no message")?;
    let method_hash: [u8; 32] = Sha256::digest(6_i32.to_le_bytes()).into();
    if source_messages.len() != 1
        || source.content.attachments.len() != 5
        || !source.complete
        || source.content.attachments.iter().any(|attachment| {
            attachment.streamed_size == 0
                || !attachment
                    .rendering_properties
                    .iter()
                    .any(|property| property.id == 0x3701 && property.value_type == Some(0x000D))
                || !attachment.rendering_properties.iter().any(|property| {
                    property.id == 0x3705
                        && property.value_type == Some(0x0003)
                        && property.sha256 == method_hash
                })
        })
    {
        return Err(format!(
            "Outlook OLE source does not match the five-object contract: {source:?}"
        )
        .into());
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
    if output.stdout.is_empty() {
        return Err(format!(
            "Outlook OLE split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let generated = job.join("parts/part-0001.pst");
    let generated_messages = independent_messages(&generated)?;
    let generated_message = generated_messages
        .first()
        .ok_or("generated Outlook OLE output has no message")?;
    let source_rtf = source
        .content
        .body_properties
        .iter()
        .find(|property| property.id == 0x1009)
        .ok_or("Outlook OLE source has no compressed RTF body")?;
    let generated_rtf = generated_message
        .content
        .body_properties
        .iter()
        .find(|property| property.id == 0x1009)
        .ok_or("generated Outlook OLE output lost its compressed RTF body")?;
    if generated_rtf != source_rtf {
        return Err("Outlook OLE compressed RTF body changed".into());
    }
    let ole_contract = |attachments: &[AttachmentFingerprint]| {
        attachments
            .iter()
            .map(|attachment| {
                (
                    attachment.index,
                    attachment.streamed_size,
                    attachment.sha256,
                    attachment
                        .rendering_properties
                        .iter()
                        .filter(|property| {
                            matches!(property.id, 0x3701 | 0x3702 | 0x3705 | 0x3709 | 0x370A)
                        })
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    if ole_contract(&generated_message.content.attachments)
        != ole_contract(&source.content.attachments)
    {
        return Err("Outlook OLE method, type, metadata, or payload changed".into());
    }
    if report["written_candidates"].as_u64() != Some(1)
        || report["parts"].as_array().map(Vec::len) != Some(1)
        || report["parts"][0]["omitted_attachments"].as_u64() != Some(0)
    {
        return Err(
            format!("Outlook OLE split did not preserve every attachment: {report}").into(),
        );
    }
    if pstforge_core::SourceFile::open(&case.path)?.identity() != &identity {
        return Err("Outlook OLE source identity changed during the split".into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the external v042-calendar-exception-source corpus case"]
fn milestone_0_4_2_calendar_exceptions_roundtrip_through_libpff()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let case = manifest
        .cases
        .iter()
        .find(|case| case.name == "v042-calendar-exception-source")
        .ok_or("manifest has no v042-calendar-exception-source case")?;
    let identity = pstforge_core::SourceFile::open(&case.path)?
        .identity()
        .clone();
    if identity.sha256.as_deref() != Some(case.sha256.as_str()) {
        return Err("calendar-exception source SHA-256 does not match its manifest".into());
    }

    let source_messages = independent_messages(&case.path)?;
    if source_messages.len() != 2
        || !source_messages.iter().all(|message| message.complete)
        || !source_messages.iter().any(|message| {
            message.folder_path == ["Calendar"]
                && message.content.message_class.as_deref() == Some("IPM.Appointment")
                && message.content.subject.as_deref()
                    == Some("Recurring appointment exception checkpoint")
                && message.content.attachments.len() == 1
                && message.content.attachments[0]
                    .rendering_properties
                    .iter()
                    .map(|property| property.id)
                    .collect::<BTreeSet<_>>()
                    == BTreeSet::from([
                        0x3001, 0x3701, 0x3702, 0x3705, 0x3709, 0x370b, 0x370e, 0x3714, 0x7ffa,
                        0x7ffb, 0x7ffc, 0x7ffd, 0x7ffe, 0x7fff,
                    ])
        })
        || !source_messages.iter().any(|message| {
            message.content.message_class.as_deref()
                == Some("IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}")
                && message.content.subject.as_deref()
                    == Some("Modified recurrence instance checkpoint")
                && message.content.embedded_path == [0]
        })
    {
        return Err("calendar-exception source does not match the structural contract".into());
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
            "calendar-exception split failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if report["partial"].as_bool() != Some(false)
        || report["written_candidates"].as_u64() != Some(2)
        || report["parts"].as_array().map(Vec::len) != Some(1)
        || report["parts"][0]["omitted_folders"].as_u64() != Some(0)
        || report["parts"][0]["omitted_properties"].as_u64() != Some(0)
        || report["parts"][0]["omitted_attachments"].as_u64() != Some(0)
    {
        return Err("calendar-exception split did not report complete preservation".into());
    }
    let generated = job.join("parts/part-0001.pst");
    verify_exact_message_fidelity(source_messages, independent_messages(&generated)?)?;
    if independent_folder_classes(&generated)?
        .get(&vec!["Calendar".to_owned()])
        .map(String::as_str)
        != Some("IPF.Appointment")
    {
        return Err("generated calendar folder is not IPF.Appointment".into());
    }
    if pstforge_core::SourceFile::open(&case.path)?.identity() != &identity {
        return Err("calendar-exception source identity changed during the split".into());
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
        if before.sha256.as_deref() != Some(case.sha256.as_str()) {
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
                .arg("--restartable")
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
            let mut reconstructions = pstforge_job::ReconstructionCounts::default();
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
                if identity.size_bytes != byte_len || identity.sha256.as_deref() != Some(sha256) {
                    return Err(format!("{} part identity mismatch", case.name).into());
                }
                let inventory = pstforge_core::verify(&path)?;
                output_messages = output_messages.saturating_add(inventory.inventory.normal_items);
                let part_fingerprints = independent_messages(&path)?;
                let (_, replicated_all_folders) =
                    replicated_source_folder_counts(&source_messages, &part_fingerprints)?;
                output_fingerprints.extend(part_fingerprints);
                let store = pstforge_pst::open_store(&path)?;
                let record_key = lower_hex(store.properties().record_key()?.record_key());
                let sidecar_name = format!("{}.json", filename.trim_end_matches(".pst"));
                let sidecar_bytes = fs::read(job.join(".pstforge/manifests").join(sidecar_name))?;
                let sidecar: pstforge_job::PartSidecar = serde_json::from_slice(&sidecar_bytes)?;
                reconstructions.merge(sidecar.reconstructions.clone());
                let expected_inventory_folders = sidecar
                    .folder_count
                    .saturating_add(WRITER_MANDATORY_FOLDER_COUNT);
                if sidecar.folder_count < replicated_all_folders
                    || inventory.inventory.folders != expected_inventory_folders
                {
                    return Err(format!(
                        "{} folder accounting mismatch: sidecar={}, required message paths={}, inventory={}, expected inventory={}",
                        case.name,
                        sidecar.folder_count,
                        replicated_all_folders,
                        inventory.inventory.folders,
                        expected_inventory_folders
                    )
                    .into());
                }
                let published_metadata = fs::metadata(&path)?;
                if sidecar.schema_version != "1.2.0"
                    || sidecar.producer_version != env!("CARGO_PKG_VERSION")
                    || u64::from(sidecar.index) != part["index"].as_u64().unwrap_or_default()
                    || sidecar.filename != filename
                    || sidecar.byte_len != byte_len
                    || sidecar.sha256.as_deref() != Some(sha256)
                    || sidecar.oversize != oversize
                    || Some(sidecar.folder_count) != part["folder_count"].as_u64()
                    || Some(sidecar.message_count) != part["message_count"].as_u64()
                    || sidecar.message_count != inventory.inventory.normal_items
                    || Some(sidecar.partial) != part["partial"].as_bool()
                    || Some(sidecar.omitted_properties) != part["omitted_properties"].as_u64()
                    || Some(sidecar.omitted_attachments) != part["omitted_attachments"].as_u64()
                    || sidecar.store_record_key != record_key
                    || sidecar.published_device != Some(published_metadata.dev())
                    || sidecar.published_inode != Some(published_metadata.ino())
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
            if reconstructions.is_empty() {
                return Err(format!(
                    "{} normalized missing metadata without reconstruction accounting",
                    case.name
                )
                .into());
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
                .arg("--restartable")
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
                .arg("--restartable")
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
#[ignore = "requires PSTFORGE_REPAIR_CORPUS_MANIFEST and PSTFORGE_REPAIR_SCRATCH"]
fn milestone_0_4_6_corrupt_sources_match_repaired_references()
-> Result<(), Box<dyn std::error::Error>> {
    let (manifest_path, scratch) = match (
        std::env::var_os("PSTFORGE_REPAIR_CORPUS_MANIFEST"),
        std::env::var_os("PSTFORGE_REPAIR_SCRATCH"),
    ) {
        (None, None) => {
            eprintln!("historical repair corpus ... skipped (private passing pairs removed)");
            return Ok(());
        }
        (Some(manifest), Some(scratch)) => (manifest, PathBuf::from(scratch)),
        _ => {
            return Err(
                "PSTFORGE_REPAIR_CORPUS_MANIFEST and PSTFORGE_REPAIR_SCRATCH must be set together"
                    .into(),
            );
        }
    };
    if !scratch.is_dir() {
        return Err("PSTFORGE_REPAIR_SCRATCH must be an existing directory".into());
    }
    let manifest: RepairManifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    if manifest.schema_version != 1 {
        return Err("repair corpus manifest schema_version must be 1".into());
    }
    if manifest.cases.is_empty() {
        return Err("repair corpus manifest contains no cases".into());
    }
    let requested_case = std::env::var("PSTFORGE_REPAIR_CASE").ok();
    let mut names = BTreeSet::new();
    let mut selected = 0_usize;
    for case in &manifest.cases {
        if case.name.is_empty() || !names.insert(case.name.as_str()) {
            return Err("repair corpus case names must be nonempty and unique".into());
        }
        if requested_case
            .as_deref()
            .is_some_and(|requested| requested != case.name)
        {
            continue;
        }
        selected = selected.saturating_add(1);
        let supplements = case
            .source_supplements
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if supplements.len() != case.source_supplements.len()
            || supplements.iter().any(|value| {
                !matches!(
                    *value,
                    "recipients" | "body properties" | "associated items"
                )
            })
        {
            return Err(format!("{} source supplements are invalid", case.name).into());
        }
        let source = pstforge_core::SourceFile::open(&case.source_path)?;
        let source_before = source.identity().clone();
        let repaired = match pstforge_core::SourceFile::open(&case.repaired_path) {
            Ok(repaired) => repaired,
            Err(error) => {
                let source_immutability = (|| -> Result<(), Box<dyn std::error::Error>> {
                    source.verify_unchanged()?;
                    if source.identity() != &source_before {
                        return Err(format!("{} source identity changed", case.name).into());
                    }
                    Ok(())
                })();
                return finish_repair_immutability(
                    &case.name,
                    Err(Box::new(error)),
                    source_immutability,
                );
            }
        };
        let repaired_before = repaired.identity().clone();
        let identity_result = (|| -> Result<(), Box<dyn std::error::Error>> {
            if source_before.sha256.as_deref() != Some(case.source_sha256.as_str()) {
                return Err(format!("{} corrupt-source SHA-256 mismatch", case.name).into());
            }
            if repaired_before.sha256.as_deref() != Some(case.repaired_sha256.as_str()) {
                return Err(format!("{} repaired-reference SHA-256 mismatch", case.name).into());
            }
            Ok(())
        })();
        let identity_immutability = verify_repair_pair_unchanged(
            &case.name,
            &source,
            &source_before,
            &repaired,
            &repaired_before,
        );
        finish_repair_immutability(&case.name, identity_result, identity_immutability)?;

        let preparation_result = (|| -> Result<_, Box<dyn std::error::Error>> {
            let (expected_messages, unreadable_associated_folders) =
                independent_repaired_messages(&repaired)?;
            if expected_messages.iter().any(|message| !message.complete) {
                return Err(format!("{} repaired-reference item is incomplete", case.name).into());
            }
            Ok((expected_messages, unreadable_associated_folders))
        })();
        let preparation_immutability = verify_repair_pair_unchanged(
            &case.name,
            &source,
            &source_before,
            &repaired,
            &repaired_before,
        );
        let (expected_messages, unreadable_associated_folders) =
            finish_repair_immutability(&case.name, preparation_result, preparation_immutability)?;
        let directory_result: Result<tempfile::TempDir, Box<dyn std::error::Error>> =
            tempfile::Builder::new()
                .prefix(".pstforge-repair-")
                .tempdir_in(&scratch)
                .map_err(Into::into);
        let directory = match directory_result {
            Ok(directory) => directory,
            Err(error) => {
                let immutability = verify_repair_pair_unchanged(
                    &case.name,
                    &source,
                    &source_before,
                    &repaired,
                    &repaired_before,
                );
                return finish_repair_immutability(&case.name, Err(error), immutability);
            }
        };
        let case_result = (|| -> Result<(u64, u64, bool), Box<dyn std::error::Error>> {
            let job = directory.path().join("job");
            let output = Command::new(env!("CARGO_BIN_EXE_pstforge"))
                .arg("split")
                .arg(&case.source_path)
                .arg("--output")
                .arg(&job)
                .arg("--max-pst-size")
                .arg("64GiB")
                .arg("--json")
                .arg("--color")
                .arg("never")
                .output()?;
            if !output.status.success() && output.status.code() != Some(1) {
                return Err(format!("{} recovery command failed", case.name).into());
            }
            let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            if report["recovery"]["unsupported_candidates"].as_u64() != Some(0) {
                return Err(
                    format!("{} recovery reported unsupported candidates", case.name).into(),
                );
            }
            let parts = report["parts"]
                .as_array()
                .ok_or_else(|| format!("{} report parts are absent", case.name))?;
            if parts.len() != 1 {
                return Err(
                    format!("{} recovery did not produce exactly one part", case.name).into(),
                );
            }
            let expected_count = u64::try_from(expected_messages.len())?;
            let written_count = report["written_candidates"].as_u64().unwrap_or_default();
            if written_count < expected_count {
                return Err(format!(
                    "{} item count is below repaired reference: expected at least {}, wrote {}",
                    case.name, expected_count, written_count
                )
                .into());
            }
            let filename = parts[0]["filename"]
                .as_str()
                .ok_or_else(|| format!("{} part filename is absent", case.name))?;
            let generated = job.join("parts").join(filename);
            pstforge_core::verify(&generated)?;
            let pffinfo = Command::new("pffinfo").arg(&generated).output()?;
            if !pffinfo.status.success() {
                return Err(format!("{} generated part failed pffinfo", case.name).into());
            }

            let actual_messages = independent_generated_messages(&generated)?;
            if actual_messages.iter().any(|message| !message.complete) {
                return Err(format!("{} generated item is incomplete", case.name).into());
            }
            let value_supplements = supplements
                .iter()
                .copied()
                .filter(|supplement| *supplement != "associated items")
                .collect::<BTreeSet<_>>();
            let mut source_messages = None;
            let mut consumed_sources = None;
            let (extra_messages, value_supplemented) =
                match verify_repair_message_fidelity(&expected_messages, actual_messages.clone()) {
                    Ok(extra) => {
                        if !value_supplements.is_empty() {
                            return Err(format!(
                                "{} declares an unobserved source value supplement",
                                case.name
                            )
                            .into());
                        }
                        (extra, false)
                    }
                    Err(reference_error) => {
                        if value_supplements.is_empty()
                            || reference_error.categories != value_supplements
                        {
                            return Err(format!("{}: {reference_error}", case.name).into());
                        }
                        let damaged = independent_damaged_messages(&source)
                            .map_err(|_| format!("{}: {reference_error}", case.name))?;
                        let (extra, consumed) = verify_source_supplemented_fidelity(
                            &expected_messages,
                            &damaged,
                            actual_messages,
                            &value_supplements,
                        )
                        .map_err(|_| format!("{}: {reference_error}", case.name))?;
                        source_messages = Some(damaged);
                        consumed_sources = Some(consumed);
                        (extra, true)
                    }
                };
            let extra_count = u64::try_from(extra_messages.len())?;
            let associated_supplemented = !extra_messages.is_empty();
            if supplements.contains("associated items") != associated_supplemented {
                return Err(format!(
                    "{} associated-item supplement does not match observed extras",
                    case.name
                )
                .into());
            }
            if written_count != expected_count.saturating_add(extra_count)
                || (!extra_messages.is_empty()
                    && extra_messages.iter().any(|message| {
                        !message.content.associated
                            || !unreadable_associated_folders.contains(&message.folder_identity)
                    }))
            {
                return Err(format!(
                    "{} has unexplained items beyond the repaired reference",
                    case.name
                )
                .into());
            }
            if !extra_messages.is_empty() {
                let damaged = match source_messages {
                    Some(messages) => messages,
                    None => independent_damaged_messages(&source)?,
                };
                verify_extra_messages_from_source(
                    &expected_messages,
                    &extra_messages,
                    &damaged,
                    consumed_sources,
                )
                .map_err(|error| format!("{}: {error}", case.name))?;
            }

            Ok((
                expected_count,
                extra_count,
                value_supplemented || associated_supplemented,
            ))
        })();
        let immutability_result = verify_repair_pair_unchanged(
            &case.name,
            &source,
            &source_before,
            &repaired,
            &repaired_before,
        );
        let case_result = finish_repair_immutability(&case.name, case_result, immutability_result);
        let (expected_count, extra_count, source_supplemented) =
            finish_repair_case(&case.name, directory, case_result)?;
        eprintln!(
            "repair corpus {} ... ok ({} reference items, {} extra associated items, source supplement: {})",
            case.name, expected_count, extra_count, source_supplemented
        );
    }
    if selected == 0 {
        return Err("PSTFORGE_REPAIR_CASE did not match a manifest case".into());
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
            .arg("--restartable")
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
            .arg("--restartable")
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
        .arg("--restartable")
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
        .arg("--restartable")
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
        .arg("--restartable")
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
        .arg("--restartable")
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
        .arg("--restartable")
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
        .arg("--restartable")
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
        if before_hash.as_deref() != Some(case.sha256.as_str()) {
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
    if before.sha256.as_deref() != Some(case.sha256.as_str()) {
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
fn milestone_0_4_5_writer_order_traversal_matches_direct_stream_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::env::var_os("PSTFORGE_CORPUS_MANIFEST")
        .ok_or("PSTFORGE_CORPUS_MANIFEST is required")?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let mut totals = WriterOrderSink::default();
    for case in &manifest.cases {
        let source = pstforge_core::SourceFile::open(&case.path)?;
        if source.identity().sha256.as_deref() != Some(case.sha256.as_str()) {
            return Err(format!("{} SHA-256 does not match its manifest", case.name).into());
        }
        let file = fs::File::open(&case.path)?;
        let native = libpff_sys::PffFile::open_fd(file.as_fd())?;
        let mut sink = WriterOrderSink::default();
        native.catalog(&mut sink)?;
        if !sink.message_stack.is_empty() || !sink.attachments.is_empty() {
            return Err(format!("{} left writer-order state open", case.name).into());
        }
        totals.nested_messages = totals.nested_messages.saturating_add(sink.nested_messages);
        totals.attachment_payloads = totals
            .attachment_payloads
            .saturating_add(sink.attachment_payloads);
        totals.attachment_properties = totals
            .attachment_properties
            .saturating_add(sink.attachment_properties);
        totals.message_properties = totals
            .message_properties
            .saturating_add(sink.message_properties);
        source.verify_unchanged()?;
    }
    if totals.nested_messages == 0
        || totals.attachment_payloads == 0
        || totals.attachment_properties == 0
        || totals.message_properties == 0
    {
        return Err(format!("external corpus lacks writer-order coverage: nested={}, attachment_payloads={}, attachment_properties={}, message_properties={}", totals.nested_messages, totals.attachment_payloads, totals.attachment_properties, totals.message_properties).into());
    }
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
        if before_hash.as_deref() != Some(case.sha256.as_str()) {
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
