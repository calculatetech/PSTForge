use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

use libpff_sys::{CatalogProvenance, FolderAddress, NamedPropertyIdentity, RecoveryUnit};
use pstforge_job::{
    CandidateOwnership, DurableCatalogSink, JobError, SpooledBlob, SpooledCandidate, SpooledEvent,
    SpooledFolder,
};
use thiserror::Error;

use crate::{ContentCompleteness, ItemKey, RecoveryProvenance};

const MAX_EMBEDDED_MESSAGE_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalRecipient {
    pub index: u32,
    pub recipient_type: Option<u32>,
    pub display_name: Option<String>,
    pub email_address: Option<String>,
    pub address_type: Option<String>,
    pub properties: Vec<CanonicalProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProperty {
    pub owner: String,
    pub owner_index: Option<u32>,
    pub record_set_index: u32,
    pub entry_index: u32,
    pub property_id: Option<u32>,
    pub value_type: Option<u32>,
    pub named_property: Option<NamedPropertyIdentity>,
    pub blob: SpooledBlob,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalAttachment {
    pub index: u32,
    pub attachment_type: Option<i32>,
    pub filename: Option<String>,
    pub declared_size: Option<u64>,
    pub data: Option<SpooledBlob>,
    pub data_complete: bool,
    pub properties: Vec<CanonicalProperty>,
    pub embedded: Option<Box<CanonicalMail>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalMail {
    pub durable_item_key: String,
    pub key: ItemKey,
    pub folder_path: Vec<String>,
    pub folder_location: CanonicalFolderLocation,
    pub folder_role: CanonicalFolderRole,
    pub placement: CanonicalMessagePlacement,
    pub message_class: Option<String>,
    pub subject: Option<String>,
    pub sender_name: Option<String>,
    pub sender_email: Option<String>,
    pub submit_filetime: Option<u64>,
    pub delivery_filetime: Option<u64>,
    pub recipients: Vec<CanonicalRecipient>,
    pub attachments: Vec<CanonicalAttachment>,
    pub properties: Vec<CanonicalProperty>,
    pub completeness: ContentCompleteness,
    pub spooled_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalFolder {
    pub path: Vec<String>,
    pub location: CanonicalFolderLocation,
    pub role: CanonicalFolderRole,
    pub container_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalFolderSet {
    pub folders: Vec<CanonicalFolder>,
    pub omitted_folders: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanonicalFolderRole {
    #[default]
    Ordinary,
    DeletedItems,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanonicalFolderLocation {
    StoreRoot,
    #[default]
    IpmSubtree,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanonicalMessagePlacement {
    #[default]
    Normal,
    Associated,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AttachmentTerminal {
    Payload,
    Missing,
}

#[derive(serde::Deserialize)]
struct OwnershipMetadata {
    parent_message_id: Option<u32>,
    parent_attachment_index: Option<u32>,
    embedded_path: Vec<u32>,
}

#[derive(Debug, Error)]
pub enum CanonicalError {
    #[error(transparent)]
    Job(#[from] JobError),
    #[error("invalid durable candidate {item_key}: {detail}")]
    InvalidCandidate { item_key: String, detail: String },
    #[error("folder hierarchy is cyclic or deeper than 64 at source folder {0}")]
    InvalidFolderHierarchy(u32),
    #[error("embedded candidate ownership is cyclic at {0}")]
    EmbeddedCycle(String),
    #[error("size accounting overflow")]
    SizeOverflow,
}

pub fn load_canonical_mail(job: &DurableCatalogSink) -> Result<Vec<CanonicalMail>, CanonicalError> {
    load_canonical_mail_expected(job, None)
}

pub fn load_canonical_folders(
    job: &DurableCatalogSink,
) -> Result<CanonicalFolderSet, CanonicalError> {
    load_canonical_folders_expected(job, None)
}

pub fn load_canonical_folders_interruptible(
    job: &DurableCatalogSink,
    interrupted: &AtomicBool,
) -> Result<CanonicalFolderSet, CanonicalError> {
    load_canonical_folders_expected(job, Some(interrupted))
}

fn load_canonical_folders_expected(
    job: &DurableCatalogSink,
    interrupted: Option<&AtomicBool>,
) -> Result<CanonicalFolderSet, CanonicalError> {
    const NID_IPM_SUBTREE: u32 = 0x8022;
    const NID_DELETED_ITEMS: u32 = 0x8062;

    let folders = job.spooled_folders()?;
    let mut by_address = BTreeMap::<FolderAddress, &SpooledFolder>::new();
    for folder in &folders {
        if let Some(address) = folder.address {
            by_address.insert(address, folder);
        }
    }
    let ipm_addresses = folders
        .iter()
        .filter(|folder| {
            folder.source_id == NID_IPM_SUBTREE
                && folder
                    .address
                    .and_then(FolderAddress::parent)
                    .is_some_and(|parent| parent == FolderAddress::root())
        })
        .filter_map(|folder| folder.address)
        .collect::<BTreeSet<_>>();
    let deleted_items_address = folders
        .iter()
        .filter(|folder| folder.source_id == NID_DELETED_ITEMS)
        .filter_map(|folder| folder.address)
        .filter(|address| {
            address
                .parent()
                .is_some_and(|parent| ipm_addresses.contains(&parent))
        })
        .min();
    let mut by_path = BTreeMap::<(CanonicalFolderLocation, Vec<String>), CanonicalFolder>::new();
    let mut omitted_folders = 0_u64;
    for folder in folders.iter().filter(|folder| {
        folder
            .address
            .is_some_and(|address| !ipm_addresses.contains(&address))
    }) {
        if interrupted.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(JobError::Interrupted.into());
        }
        let mut path = Vec::new();
        let mut current = folder.address;
        let mut seen = BTreeSet::new();
        let mut location = None;
        while let Some(address) = current {
            if ipm_addresses.contains(&address) {
                location = Some(CanonicalFolderLocation::IpmSubtree);
                break;
            }
            if address == FolderAddress::root() {
                location = Some(CanonicalFolderLocation::StoreRoot);
                break;
            }
            if path.len() == 64 || !seen.insert(address) {
                return Err(CanonicalError::InvalidFolderHierarchy(folder.source_id));
            }
            let Some(current_folder) = by_address.get(&address) else {
                return Err(CanonicalError::InvalidFolderHierarchy(folder.source_id));
            };
            path.push(
                current_folder
                    .name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("Recovered Folder {}", current_folder.source_id)),
            );
            current = address.parent();
        }
        let Some(location) = location else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        if location == CanonicalFolderLocation::StoreRoot
            && (folder.source_id & 0x1f != 0x02 || folder.source_id == 0x8042)
        {
            continue;
        }
        path.reverse();
        let candidate = CanonicalFolder {
            path,
            location,
            role: if folder.address == deleted_items_address {
                CanonicalFolderRole::DeletedItems
            } else {
                CanonicalFolderRole::Ordinary
            },
            container_class: folder.container_class.clone(),
        };
        match by_path.entry((candidate.location, candidate.path.clone())) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                omitted_folders = omitted_folders.saturating_add(1);
                if candidate.role == CanonicalFolderRole::DeletedItems {
                    entry.insert(candidate);
                }
            }
        }
    }
    Ok(CanonicalFolderSet {
        folders: by_path.into_values().collect(),
        omitted_folders,
    })
}

pub fn load_canonical_mail_interruptible(
    job: &DurableCatalogSink,
    interrupted: &AtomicBool,
) -> Result<Vec<CanonicalMail>, CanonicalError> {
    load_canonical_mail_expected(job, Some(interrupted))
}

fn load_canonical_mail_expected(
    job: &DurableCatalogSink,
    interrupted: Option<&AtomicBool>,
) -> Result<Vec<CanonicalMail>, CanonicalError> {
    let check_interrupted = || {
        if interrupted.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            Err(CanonicalError::Job(JobError::Interrupted))
        } else {
            Ok(())
        }
    };
    check_interrupted()?;
    let folders = job.spooled_folders()?;
    let folders_by_address = folders
        .iter()
        .filter_map(|folder| folder.address.map(|address| (address, folder)))
        .collect::<BTreeMap<_, _>>();
    let mut folders_by_id = BTreeMap::<u32, Option<&SpooledFolder>>::new();
    for folder in &folders {
        folders_by_id
            .entry(folder.source_id)
            .and_modify(|value| *value = None)
            .or_insert(Some(folder));
    }
    let candidates = match interrupted {
        Some(flag) => job.spooled_candidates_interruptible(flag)?,
        None => job.spooled_candidates()?,
    };
    let by_key = candidates
        .iter()
        .map(|candidate| (candidate.item_key.as_str(), candidate))
        .collect::<BTreeMap<_, _>>();
    let ownerships = match interrupted {
        Some(flag) => job.candidate_ownerships_interruptible(flag)?,
        None => job.candidate_ownerships()?,
    };
    let ownership_by_key = ownerships
        .iter()
        .map(|ownership| (ownership.item_key.as_str(), ownership))
        .collect::<BTreeMap<_, _>>();
    let mut claimed_slots = BTreeMap::<(&str, u32), &CandidateOwnership>::new();
    for ownership in &ownerships {
        check_interrupted()?;
        validate_ownership(ownership, &ownership_by_key)?;
        if let (Some(parent), Some(index)) = (
            ownership.parent_item_key.as_deref(),
            ownership.parent_attachment_index,
        ) {
            if let Some(previous) = claimed_slots.insert((parent, index), ownership) {
                return Err(CanonicalError::InvalidCandidate {
                    item_key: ownership.item_key.clone(),
                    detail: format!(
                        "embedded attachment slot is also claimed by {}",
                        previous.item_key
                    ),
                });
            }
        }
    }
    let mut children = BTreeMap::<(&str, u32), &SpooledCandidate>::new();
    for candidate in &candidates {
        check_interrupted()?;
        let ownership = ownership_by_key
            .get(candidate.item_key.as_str())
            .ok_or_else(|| {
                invalid_error(candidate, "writable candidate has no ownership record")
            })?;
        if !ownership.writable
            || ownership.parent_item_key != candidate.parent_item_key
            || ownership.parent_attachment_index != candidate.parent_attachment_index
        {
            return invalid(candidate, "writable candidate ownership is inconsistent");
        }
        if let (Some(parent), Some(index)) = (
            candidate.parent_item_key.as_deref(),
            candidate.parent_attachment_index,
        ) {
            if !ownership_by_key
                .get(parent)
                .is_some_and(|ownership| ownership.writable)
            {
                continue;
            }
            children.insert((parent, index), candidate);
        }
    }
    let mut unsupported_children = BTreeMap::<&str, BTreeSet<u32>>::new();
    for ownership in ownerships.iter().filter(|ownership| !ownership.writable) {
        check_interrupted()?;
        let (Some(parent_key), Some(index)) = (
            ownership.parent_item_key.as_deref(),
            ownership.parent_attachment_index,
        ) else {
            continue;
        };
        if ownership_by_key
            .get(parent_key)
            .is_some_and(|parent| parent.writable)
        {
            unsupported_children
                .entry(parent_key)
                .or_default()
                .insert(index);
        }
    }

    let mut output = Vec::new();
    for candidate in candidates.iter().filter(|candidate| {
        candidate
            .parent_item_key
            .as_deref()
            .is_none_or(|parent| !by_key.contains_key(parent))
    }) {
        check_interrupted()?;
        output.push(build_mail(
            candidate,
            &folders_by_address,
            &folders_by_id,
            &children,
            &unsupported_children,
            &mut BTreeSet::new(),
            None,
        )?);
    }
    let accounted = count_mail(&output)?;
    if accounted != candidates.len() {
        return Err(CanonicalError::InvalidCandidate {
            item_key: "<job>".to_owned(),
            detail: "not every writable candidate is reachable from a top-level message".to_owned(),
        });
    }
    Ok(output)
}

fn validate_ownership(
    ownership: &CandidateOwnership,
    by_key: &BTreeMap<&str, &CandidateOwnership>,
) -> Result<(), CanonicalError> {
    if ownership.embedded_path.len() > MAX_EMBEDDED_MESSAGE_DEPTH {
        return invalid_ownership(
            ownership,
            "embedded path exceeds the supported depth of 256",
        );
    }
    let metadata: OwnershipMetadata =
        serde_json::from_value(ownership.metadata.clone()).map_err(|error| {
            CanonicalError::InvalidCandidate {
                item_key: ownership.item_key.clone(),
                detail: format!("invalid ownership metadata: {error}"),
            }
        })?;
    if metadata.parent_attachment_index != ownership.parent_attachment_index
        || metadata.embedded_path != ownership.embedded_path
    {
        return invalid_ownership(
            ownership,
            "ownership columns disagree with message metadata",
        );
    }
    match (
        ownership.parent_item_key.as_deref(),
        ownership.parent_attachment_index,
    ) {
        (None, None) => {
            if metadata.parent_message_id.is_some() || !ownership.embedded_path.is_empty() {
                return invalid_ownership(ownership, "top-level candidate has embedded ownership");
            }
        }
        (Some(parent_key), Some(index)) => {
            let parent =
                by_key
                    .get(parent_key)
                    .ok_or_else(|| CanonicalError::InvalidCandidate {
                        item_key: ownership.item_key.clone(),
                        detail: "embedded parent is not durably committed".to_owned(),
                    })?;
            let mut expected_path = parent.embedded_path.clone();
            expected_path.push(index);
            if ownership.embedded_path != expected_path {
                return invalid_ownership(ownership, "embedded path disagrees with its parent");
            }
            if metadata.parent_message_id != Some(parent.source_node_id.unwrap_or(0)) {
                return invalid_ownership(
                    ownership,
                    "parent message id disagrees with its durable parent",
                );
            }
        }
        _ => return invalid_ownership(ownership, "embedded ownership is incomplete"),
    }
    Ok(())
}

fn invalid_ownership<T>(ownership: &CandidateOwnership, detail: &str) -> Result<T, CanonicalError> {
    Err(CanonicalError::InvalidCandidate {
        item_key: ownership.item_key.clone(),
        detail: detail.to_owned(),
    })
}

fn build_mail(
    candidate: &SpooledCandidate,
    folders_by_address: &BTreeMap<FolderAddress, &SpooledFolder>,
    folders_by_id: &BTreeMap<u32, Option<&SpooledFolder>>,
    children: &BTreeMap<(&str, u32), &SpooledCandidate>,
    unsupported_children: &BTreeMap<&str, BTreeSet<u32>>,
    active: &mut BTreeSet<String>,
    inherited_folder: Option<(&[String], CanonicalFolderLocation, CanonicalFolderRole)>,
) -> Result<CanonicalMail, CanonicalError> {
    if !active.insert(candidate.item_key.clone()) {
        return Err(CanonicalError::EmbeddedCycle(candidate.item_key.clone()));
    }
    let result = build_mail_inner(
        candidate,
        folders_by_address,
        folders_by_id,
        children,
        unsupported_children,
        active,
        inherited_folder,
    );
    active.remove(&candidate.item_key);
    result
}

fn build_mail_inner(
    candidate: &SpooledCandidate,
    folders_by_address: &BTreeMap<FolderAddress, &SpooledFolder>,
    folders_by_id: &BTreeMap<u32, Option<&SpooledFolder>>,
    children: &BTreeMap<(&str, u32), &SpooledCandidate>,
    unsupported_children: &BTreeMap<&str, BTreeSet<u32>>,
    active: &mut BTreeSet<String>,
    inherited_folder: Option<(&[String], CanonicalFolderLocation, CanonicalFolderRole)>,
) -> Result<CanonicalMail, CanonicalError> {
    let (folder_path, folder_location, folder_role) = match inherited_folder {
        Some((path, location, role)) => (path.to_vec(), location, role),
        None => (
            candidate_folder_path(candidate, folders_by_address, folders_by_id)?,
            candidate_folder_location(candidate, folders_by_address, folders_by_id),
            candidate_folder_role(candidate, folders_by_address, folders_by_id),
        ),
    };
    let mut recipients = BTreeMap::new();
    let mut attachments = BTreeMap::<u32, CanonicalAttachment>::new();
    let mut attachment_terminals = BTreeMap::new();
    let mut properties = Vec::new();
    let mut spooled_bytes = 0_u64;
    for event in &candidate.events {
        if let Some(blob) = &event.blob {
            spooled_bytes = spooled_bytes
                .checked_add(blob.byte_len)
                .ok_or(CanonicalError::SizeOverflow)?;
        }
        match event.kind.as_str() {
            "recipient" => {
                let index = required_u32(candidate, event, "index")?;
                if recipients
                    .insert(
                        index,
                        CanonicalRecipient {
                            index,
                            recipient_type: optional_u32(candidate, event, "recipient_type")?,
                            display_name: optional_string(candidate, event, "display_name")?,
                            email_address: optional_string(candidate, event, "email_address")?,
                            address_type: optional_string(candidate, event, "address_type")?,
                            properties: Vec::new(),
                        },
                    )
                    .is_some()
                {
                    return invalid(candidate, "duplicate recipient index");
                }
            }
            "attachment" => {
                let index = required_u32(candidate, event, "index")?;
                if attachments
                    .insert(
                        index,
                        CanonicalAttachment {
                            index,
                            attachment_type: optional_i32(candidate, event, "attachment_type")?,
                            filename: optional_string(candidate, event, "filename")?,
                            declared_size: optional_u64(candidate, event, "data_size")?,
                            data: None,
                            data_complete: false,
                            properties: Vec::new(),
                            embedded: None,
                        },
                    )
                    .is_some()
                {
                    return invalid(candidate, "duplicate attachment index");
                }
            }
            "attachment_data" | "attachment_partial" => {
                let index = required_u32(candidate, event, "index")?;
                validate_event_message(candidate, event)?;
                if attachment_terminals
                    .insert(index, AttachmentTerminal::Payload)
                    .is_some()
                {
                    return invalid(candidate, "duplicate attachment terminal state");
                }
                let actual_size = required_u64(candidate, event, "actual_size")?;
                let declared_size = optional_u64(candidate, event, "declared_size")?;
                let blob = event
                    .blob
                    .as_ref()
                    .ok_or_else(|| invalid_error(candidate, "attachment data event has no blob"))?;
                if actual_size != blob.byte_len {
                    return invalid(candidate, "attachment event length disagrees with its blob");
                }
                if event.kind == "attachment_data" && declared_size != Some(actual_size) {
                    return invalid(
                        candidate,
                        "complete attachment length disagrees with its declaration",
                    );
                }
                if declared_size.is_some_and(|declared| actual_size > declared) {
                    return invalid(candidate, "partial attachment exceeds its declaration");
                }
                let attachment = attachments
                    .get_mut(&index)
                    .ok_or_else(|| invalid_error(candidate, "attachment data precedes metadata"))?;
                if declared_size != attachment.declared_size {
                    return invalid(
                        candidate,
                        "attachment event declaration disagrees with its metadata",
                    );
                }
                if attachment.data.is_some() {
                    return invalid(candidate, "duplicate attachment data");
                }
                attachment.data = event.blob.clone();
                attachment.data_complete = event.kind == "attachment_data";
            }
            "attachment_missing" => {
                let index = required_u32(candidate, event, "index")?;
                validate_event_message(candidate, event)?;
                if attachment_terminals
                    .insert(index, AttachmentTerminal::Missing)
                    .is_some()
                {
                    return invalid(candidate, "duplicate attachment terminal state");
                }
                let declared_size = optional_u64(candidate, event, "declared_size")?;
                let attachment = attachments.get_mut(&index).ok_or_else(|| {
                    invalid_error(candidate, "missing attachment precedes metadata")
                })?;
                if declared_size != attachment.declared_size {
                    return invalid(
                        candidate,
                        "missing attachment declaration disagrees with its metadata",
                    );
                }
                attachment.data_complete = false;
            }
            "property" => {
                let property = canonical_property(candidate, event)?;
                match property.owner.as_str() {
                    "attachment" => {
                        let index = property.owner_index.ok_or_else(|| {
                            invalid_error(candidate, "attachment property has no attachment index")
                        })?;
                        attachments
                            .get_mut(&index)
                            .ok_or_else(|| {
                                invalid_error(candidate, "attachment property precedes metadata")
                            })?
                            .properties
                            .push(property);
                    }
                    "message" => properties.push(property),
                    "recipient" => {
                        let index = property.owner_index.ok_or_else(|| {
                            invalid_error(candidate, "recipient property has no recipient index")
                        })?;
                        recipients
                            .get_mut(&index)
                            .ok_or_else(|| {
                                invalid_error(candidate, "recipient property precedes metadata")
                            })?
                            .properties
                            .push(property);
                    }
                    _ => return invalid(candidate, "invalid property owner"),
                }
            }
            "property_incomplete" => {
                if event.blob.is_some() {
                    return invalid(candidate, "incomplete property has a blob");
                }
                let descriptor = event.metadata.get("property").ok_or_else(|| {
                    invalid_error(candidate, "incomplete property has no descriptor")
                })?;
                let (owner, owner_index) = validate_property_owner(candidate, descriptor)?;
                match (owner.as_str(), owner_index) {
                    ("message", None) => {}
                    ("recipient", Some(index)) if recipients.contains_key(&index) => {}
                    ("attachment", Some(index)) if attachments.contains_key(&index) => {}
                    ("recipient", Some(_)) => {
                        return invalid(candidate, "incomplete property owns unknown recipient");
                    }
                    ("attachment", Some(_)) => {
                        return invalid(candidate, "incomplete property owns unknown attachment");
                    }
                    _ => return invalid(candidate, "invalid incomplete property owner"),
                }
            }
            other => return invalid(candidate, &format!("unknown durable event kind {other:?}")),
        }
    }
    for (index, attachment) in &mut attachments {
        let child = children.get(&(candidate.item_key.as_str(), *index));
        let unsupported_child = unsupported_children
            .get(candidate.item_key.as_str())
            .is_some_and(|indices| indices.contains(index));
        let terminal = attachment_terminals.get(index).copied();
        let embedded_type = attachment.attachment_type == Some(i32::from(b'i'));
        if embedded_type && terminal == Some(AttachmentTerminal::Payload) {
            return invalid(candidate, "embedded attachment has binary payload state");
        }
        if embedded_type
            && child.is_none()
            && !unsupported_child
            && terminal != Some(AttachmentTerminal::Missing)
        {
            return invalid(
                candidate,
                "embedded attachment has no child or missing state",
            );
        }
        if !embedded_type && child.is_some() {
            return invalid(candidate, "non-embedded attachment owns an embedded child");
        }
        if !embedded_type && unsupported_child {
            return invalid(
                candidate,
                "non-embedded attachment owns an unsupported embedded child",
            );
        }
        if !embedded_type && terminal.is_none() {
            return invalid(
                candidate,
                "non-embedded attachment has no terminal payload state",
            );
        }
        if let Some(child) = child {
            if terminal.is_some() {
                return invalid(
                    candidate,
                    "embedded attachment also has a payload terminal state",
                );
            }
            let embedded = build_mail(
                child,
                folders_by_address,
                folders_by_id,
                children,
                unsupported_children,
                active,
                Some((&folder_path, folder_location, folder_role)),
            )?;
            spooled_bytes = spooled_bytes
                .checked_add(embedded.spooled_bytes)
                .ok_or(CanonicalError::SizeOverflow)?;
            attachment.embedded = Some(Box::new(embedded));
        } else if unsupported_child && terminal.is_some() {
            return invalid(
                candidate,
                "unsupported embedded child also has a terminal state",
            );
        }
    }
    Ok(CanonicalMail {
        durable_item_key: candidate.item_key.clone(),
        key: ItemKey {
            provenance: provenance(candidate.provenance),
            source_node_id: candidate.source_node_id,
            recovery_index: candidate.recovery_index,
            occurrence: candidate.occurrence,
        },
        folder_path,
        folder_location,
        folder_role,
        placement: if metadata_bool(candidate, "associated")? {
            CanonicalMessagePlacement::Associated
        } else {
            CanonicalMessagePlacement::Normal
        },
        message_class: metadata_string(candidate, "message_class")?,
        subject: metadata_string(candidate, "subject")?,
        sender_name: metadata_string(candidate, "sender_name")?,
        sender_email: metadata_string(candidate, "sender_email")?,
        submit_filetime: metadata_u64(candidate, "submit_filetime")?,
        delivery_filetime: metadata_u64(candidate, "delivery_filetime")?,
        recipients: recipients.into_values().collect(),
        attachments: attachments.into_values().collect(),
        properties,
        completeness: match candidate.completeness.as_str() {
            "complete" => ContentCompleteness::Complete,
            "partial" => ContentCompleteness::Partial,
            "damaged" => ContentCompleteness::Damaged,
            _ => return invalid(candidate, "invalid completeness"),
        },
        spooled_bytes,
    })
}

fn candidate_folder_role(
    candidate: &SpooledCandidate,
    folders_by_address: &BTreeMap<FolderAddress, &SpooledFolder>,
    folders_by_id: &BTreeMap<u32, Option<&SpooledFolder>>,
) -> CanonicalFolderRole {
    let address = match candidate.unit {
        Some(RecoveryUnit::Normal { folder, .. } | RecoveryUnit::Associated { folder, .. }) => {
            Some(folder)
        }
        _ => metadata_u32(candidate, "folder_id")
            .ok()
            .flatten()
            .and_then(|id| folders_by_id.get(&id).and_then(|value| *value))
            .and_then(|value| value.address),
    };
    if address == well_known_deleted_items_address(folders_by_address) {
        CanonicalFolderRole::DeletedItems
    } else {
        CanonicalFolderRole::Ordinary
    }
}

fn candidate_folder_location(
    candidate: &SpooledCandidate,
    folders_by_address: &BTreeMap<FolderAddress, &SpooledFolder>,
    folders_by_id: &BTreeMap<u32, Option<&SpooledFolder>>,
) -> CanonicalFolderLocation {
    let address = match candidate.unit {
        Some(RecoveryUnit::Normal { folder, .. } | RecoveryUnit::Associated { folder, .. }) => {
            Some(folder)
        }
        _ => metadata_u32(candidate, "folder_id")
            .ok()
            .flatten()
            .and_then(|id| folders_by_id.get(&id).and_then(|value| *value))
            .and_then(|value| value.address),
    };
    let Some(mut current) = address else {
        return CanonicalFolderLocation::IpmSubtree;
    };
    while let Some(parent) = current.parent() {
        if folders_by_address
            .get(&current)
            .is_some_and(|folder| folder.source_id == 0x8022)
        {
            return CanonicalFolderLocation::IpmSubtree;
        }
        if parent == FolderAddress::root() {
            return CanonicalFolderLocation::StoreRoot;
        }
        current = parent;
    }
    CanonicalFolderLocation::IpmSubtree
}

fn well_known_deleted_items_address(
    folders_by_address: &BTreeMap<FolderAddress, &SpooledFolder>,
) -> Option<FolderAddress> {
    const NID_IPM_SUBTREE: u32 = 0x8022;
    const NID_DELETED_ITEMS: u32 = 0x8062;

    folders_by_address
        .iter()
        .filter(|(_, folder)| folder.source_id == NID_DELETED_ITEMS)
        .filter_map(|(address, _)| {
            let parent = address.parent()?;
            let ipm = folders_by_address.get(&parent)?;
            (ipm.source_id == NID_IPM_SUBTREE
                && parent
                    .parent()
                    .is_some_and(|root| root == FolderAddress::root()))
            .then_some(*address)
        })
        .min()
}

fn candidate_folder_path(
    candidate: &SpooledCandidate,
    folders_by_address: &BTreeMap<FolderAddress, &SpooledFolder>,
    folders_by_id: &BTreeMap<u32, Option<&SpooledFolder>>,
) -> Result<Vec<String>, CanonicalError> {
    if let Some(RecoveryUnit::Normal { folder, .. } | RecoveryUnit::Associated { folder, .. }) =
        candidate.unit
    {
        let mut path = Vec::new();
        let mut current = Some(folder);
        while let Some(address) = current {
            if path.len() == 64 {
                return Err(CanonicalError::InvalidFolderHierarchy(
                    candidate.source_node_id.unwrap_or(0),
                ));
            }
            if let Some(folder) = folders_by_address.get(&address) {
                if is_visible_mail_folder(address, folder.source_id) {
                    path.push(
                        folder
                            .name
                            .clone()
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| format!("Recovered Folder {}", folder.source_id)),
                    );
                }
            } else if address != FolderAddress::root() {
                path.push("Recovered Folder".to_owned());
            }
            current = address.parent();
        }
        path.reverse();
        return Ok(nonempty_folder_path(path, candidate.provenance));
    }
    let Some(folder_id) = metadata_u32(candidate, "folder_id")? else {
        return Ok(vec![
            match candidate.provenance {
                CatalogProvenance::Normal => "Unfiled Mail",
                CatalogProvenance::Recovered => "Recovered Mail",
                CatalogProvenance::Orphan => "Orphan Mail",
                CatalogProvenance::Fragment => "Fragment Mail",
            }
            .to_owned(),
        ]);
    };
    let mut path = Vec::new();
    let mut current = Some(folder_id);
    let mut seen = BTreeSet::new();
    while let Some(id) = current {
        if path.len() == 64 || !seen.insert(id) {
            return Err(CanonicalError::InvalidFolderHierarchy(folder_id));
        }
        let Some(Some(folder)) = folders_by_id.get(&id) else {
            path.push(format!("Recovered Folder {id}"));
            break;
        };
        if folder
            .address
            .is_none_or(|address| is_visible_mail_folder(address, folder.source_id))
        {
            path.push(
                folder
                    .name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("Recovered Folder {id}")),
            );
        }
        current = folder.parent_source_id;
    }
    path.reverse();
    Ok(nonempty_folder_path(path, candidate.provenance))
}

fn is_visible_mail_folder(address: FolderAddress, source_id: u32) -> bool {
    const NID_IPM_SUBTREE: u32 = 0x8022;

    address != FolderAddress::root()
        && !(source_id == NID_IPM_SUBTREE
            && address
                .parent()
                .is_some_and(|parent| parent == FolderAddress::root()))
}

fn nonempty_folder_path(path: Vec<String>, provenance: CatalogProvenance) -> Vec<String> {
    if path.is_empty() {
        vec![
            match provenance {
                CatalogProvenance::Normal => "Unfiled Mail",
                CatalogProvenance::Recovered => "Recovered Mail",
                CatalogProvenance::Orphan => "Orphan Mail",
                CatalogProvenance::Fragment => "Fragment Mail",
            }
            .to_owned(),
        ]
    } else {
        path
    }
}

fn canonical_property(
    candidate: &SpooledCandidate,
    event: &SpooledEvent,
) -> Result<CanonicalProperty, CanonicalError> {
    let (owner, owner_index) = validate_property_owner(candidate, &event.metadata)?;
    let named_property = match event.metadata.get("named_property") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(serde_json::from_value(value.clone()).map_err(|error| {
            invalid_error(
                candidate,
                &format!("invalid named property identity: {error}"),
            )
        })?),
    };
    Ok(CanonicalProperty {
        owner,
        owner_index,
        record_set_index: required_u32(candidate, event, "record_set_index")?,
        entry_index: required_u32(candidate, event, "entry_index")?,
        property_id: optional_u32(candidate, event, "entry_type")?,
        value_type: optional_u32(candidate, event, "value_type")?,
        named_property,
        blob: event
            .blob
            .clone()
            .ok_or_else(|| invalid_error(candidate, "complete property has no blob"))?,
    })
}

fn validate_event_message(
    candidate: &SpooledCandidate,
    event: &SpooledEvent,
) -> Result<(), CanonicalError> {
    if required_u32(candidate, event, "message_id")? != candidate.source_node_id.unwrap_or(0) {
        return invalid(candidate, "event message id disagrees with its candidate");
    }
    Ok(())
}

fn validate_property_owner(
    candidate: &SpooledCandidate,
    metadata: &serde_json::Value,
) -> Result<(String, Option<u32>), CanonicalError> {
    let owner = optional_json_string(candidate, metadata, "owner")?
        .ok_or_else(|| invalid_error(candidate, "missing owner"))?;
    let owner_id = optional_json_u64(candidate, metadata, "owner_id")?
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid_error(candidate, "missing or invalid owner_id"))?;
    let owner_index = optional_json_u64(candidate, metadata, "owner_index")?
        .map(|value| {
            u32::try_from(value)
                .map_err(|_| invalid_error(candidate, "invalid property owner_index"))
        })
        .transpose()?;
    if owner_id != candidate.source_node_id.unwrap_or(0) {
        return invalid(candidate, "property owner id disagrees with its message");
    }
    match owner.as_str() {
        "message" if owner_index.is_none() => {}
        "recipient" | "attachment" if owner_index.is_some() => {}
        "message" => return invalid(candidate, "message property has an owner index"),
        "recipient" | "attachment" => {
            return invalid(candidate, "nested property has no owner index");
        }
        _ => return invalid(candidate, "invalid property owner"),
    }
    Ok((owner, owner_index))
}

fn count_mail(mail: &[CanonicalMail]) -> Result<usize, CanonicalError> {
    mail.iter().try_fold(0_usize, |total, message| {
        message.attachments.iter().try_fold(
            total.checked_add(1).ok_or(CanonicalError::SizeOverflow)?,
            |total, attachment| match &attachment.embedded {
                Some(embedded) => total
                    .checked_add(count_mail(std::slice::from_ref(embedded))?)
                    .ok_or(CanonicalError::SizeOverflow),
                None => Ok(total),
            },
        )
    })
}

fn provenance(value: CatalogProvenance) -> RecoveryProvenance {
    match value {
        CatalogProvenance::Normal => RecoveryProvenance::Normal,
        CatalogProvenance::Recovered => RecoveryProvenance::Recovered,
        CatalogProvenance::Orphan => RecoveryProvenance::Orphan,
        CatalogProvenance::Fragment => RecoveryProvenance::Fragment,
    }
}

fn metadata_string(
    candidate: &SpooledCandidate,
    name: &str,
) -> Result<Option<String>, CanonicalError> {
    optional_json_string(candidate, &candidate.metadata, name)
}

fn metadata_u64(candidate: &SpooledCandidate, name: &str) -> Result<Option<u64>, CanonicalError> {
    optional_json_u64(candidate, &candidate.metadata, name)
}

fn metadata_u32(candidate: &SpooledCandidate, name: &str) -> Result<Option<u32>, CanonicalError> {
    optional_json_u64(candidate, &candidate.metadata, name)?
        .map(|value| {
            u32::try_from(value).map_err(|_| invalid_error(candidate, &format!("invalid {name}")))
        })
        .transpose()
}

fn metadata_bool(candidate: &SpooledCandidate, name: &str) -> Result<bool, CanonicalError> {
    candidate
        .metadata
        .get(name)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| invalid_error(candidate, &format!("invalid or missing {name}")))
}

fn optional_string(
    candidate: &SpooledCandidate,
    event: &SpooledEvent,
    name: &str,
) -> Result<Option<String>, CanonicalError> {
    optional_json_string(candidate, &event.metadata, name)
}

fn optional_json_string(
    candidate: &SpooledCandidate,
    value: &serde_json::Value,
    name: &str,
) -> Result<Option<String>, CanonicalError> {
    match value.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        _ => invalid(candidate, &format!("invalid {name}")),
    }
}

fn required_u32(
    candidate: &SpooledCandidate,
    event: &SpooledEvent,
    name: &str,
) -> Result<u32, CanonicalError> {
    optional_u32(candidate, event, name)?
        .ok_or_else(|| invalid_error(candidate, &format!("missing {name}")))
}

fn optional_u32(
    candidate: &SpooledCandidate,
    event: &SpooledEvent,
    name: &str,
) -> Result<Option<u32>, CanonicalError> {
    optional_json_u64(candidate, &event.metadata, name)?
        .map(|value| {
            u32::try_from(value).map_err(|_| invalid_error(candidate, &format!("invalid {name}")))
        })
        .transpose()
}

fn optional_u64(
    candidate: &SpooledCandidate,
    event: &SpooledEvent,
    name: &str,
) -> Result<Option<u64>, CanonicalError> {
    optional_json_u64(candidate, &event.metadata, name)
}

fn required_u64(
    candidate: &SpooledCandidate,
    event: &SpooledEvent,
    name: &str,
) -> Result<u64, CanonicalError> {
    optional_u64(candidate, event, name)?
        .ok_or_else(|| invalid_error(candidate, &format!("missing {name}")))
}

fn optional_json_u64(
    candidate: &SpooledCandidate,
    value: &serde_json::Value,
    name: &str,
) -> Result<Option<u64>, CanonicalError> {
    match value.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| invalid_error(candidate, &format!("invalid {name}"))),
        _ => invalid(candidate, &format!("invalid {name}")),
    }
}

fn optional_i32(
    candidate: &SpooledCandidate,
    event: &SpooledEvent,
    name: &str,
) -> Result<Option<i32>, CanonicalError> {
    match event.metadata.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(value)) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| invalid_error(candidate, &format!("invalid {name}"))),
        _ => invalid(candidate, &format!("invalid {name}")),
    }
}

fn invalid<T>(candidate: &SpooledCandidate, detail: &str) -> Result<T, CanonicalError> {
    Err(invalid_error(candidate, detail))
}

fn invalid_error(candidate: &SpooledCandidate, detail: &str) -> CanonicalError {
    CanonicalError::InvalidCandidate {
        item_key: candidate.item_key.clone(),
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use libpff_sys::{
        CatalogEvent, CatalogProvenance, CatalogSink, FolderAddress, NamedPropertyIdentity,
        NamedPropertyName, PropertyDescriptor, PropertyOwner, RecoveryMode, RecoveryUnit,
    };
    use pstforge_job::DurableCatalogSink;
    use rusqlite::{Connection, params};
    use tempfile::tempdir;

    use super::{
        CanonicalFolder, CanonicalFolderLocation, CanonicalFolderRole, load_canonical_folders,
        load_canonical_mail,
    };
    use crate::{build_part_writer_input, split_recovered_job};

    fn message_start(
        sink: &mut DurableCatalogSink,
        id: u32,
        folder_id: Option<u32>,
        parent: Option<(u32, u32)>,
    ) -> Result<(), String> {
        sink.event(CatalogEvent::MessageStart {
            id,
            provenance: CatalogProvenance::Normal,
            recovery_index: None,
            folder_id,
            parent_message_id: parent.map(|value| value.0),
            parent_attachment_index: parent.map(|value| value.1),
            embedded_path: parent.map(|value| vec![value.1]).unwrap_or_default(),
            associated: false,
            item_type: Some(11),
            message_class: Some("IPM.Note".to_owned()),
            subject: Some(format!("message {id}")),
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported: true,
        })
    }

    fn property(
        sink: &mut DurableCatalogSink,
        descriptor: PropertyDescriptor,
        bytes: &[u8],
    ) -> Result<(), String> {
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyData { descriptor, bytes })?;
        sink.event(CatalogEvent::PropertyEnd(descriptor))
    }

    #[test]
    fn reconstructs_folder_recipient_and_embedded_attachment_ownership()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let mut sink = DurableCatalogSink::create(&directory.path().join("job"))?;
        let root = FolderAddress::root();
        let inbox = root.child(0).ok_or("folder address")?;
        sink.event(CatalogEvent::UnitStart(RecoveryUnit::Folder {
            address: root,
        }))?;
        sink.event(CatalogEvent::Folder {
            id: 0,
            parent_id: None,
            name: Some("Root".to_owned()),
            container_class: None,
        })?;
        sink.event(CatalogEvent::UnitEnd(RecoveryUnit::Folder {
            address: root,
        }))?;
        sink.event(CatalogEvent::UnitStart(RecoveryUnit::Folder {
            address: inbox,
        }))?;
        sink.event(CatalogEvent::Folder {
            id: 0,
            parent_id: Some(0),
            name: Some("Inbox".to_owned()),
            container_class: Some("IPF.Note".to_owned()),
        })?;
        sink.event(CatalogEvent::UnitEnd(RecoveryUnit::Folder {
            address: inbox,
        }))?;
        let message_unit = RecoveryUnit::Normal {
            folder: inbox,
            folder_id: 0,
            message_index: 0,
        };
        sink.event(CatalogEvent::UnitStart(message_unit))?;
        message_start(&mut sink, 0, Some(0), None)?;
        sink.event(CatalogEvent::Recipient {
            message_id: 0,
            index: 0,
            recipient_type: Some(1),
            display_name: Some("Recipient".to_owned()),
            email_address: None,
            address_type: None,
        })?;
        property(
            &mut sink,
            PropertyDescriptor {
                owner: PropertyOwner::Recipient {
                    message_id: 0,
                    index: 0,
                },
                record_set_index: 0,
                entry_index: 0,
                entry_type: Some(0x3001),
                value_type: Some(0x001f),
                data_size: 2,
            },
            b"R\0",
        )?;
        property(
            &mut sink,
            PropertyDescriptor {
                owner: PropertyOwner::Message(0),
                record_set_index: 0,
                entry_index: 1,
                entry_type: Some(0x10F7),
                value_type: Some(0x0003),
                data_size: 3,
            },
            b"bad",
        )?;
        let named_descriptor = PropertyDescriptor {
            owner: PropertyOwner::Message(0),
            record_set_index: 0,
            entry_index: 2,
            entry_type: Some(0x8001),
            value_type: Some(0x0003),
            data_size: 4,
        };
        sink.event(CatalogEvent::NamedProperty {
            descriptor: named_descriptor,
            identity: NamedPropertyIdentity {
                guid: [
                    0x28, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x46,
                ],
                name: NamedPropertyName::Numeric(0x6204),
            },
        })?;
        property(&mut sink, named_descriptor, &42_i32.to_le_bytes())?;
        let string_named_descriptor = PropertyDescriptor {
            owner: PropertyOwner::Message(0),
            record_set_index: 0,
            entry_index: 3,
            entry_type: Some(0x8002),
            value_type: Some(0x001F),
            data_size: 12,
        };
        sink.event(CatalogEvent::NamedProperty {
            descriptor: string_named_descriptor,
            identity: NamedPropertyIdentity {
                guid: [
                    0x29, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x46,
                ],
                name: NamedPropertyName::String("CheckpointName".to_owned()),
            },
        })?;
        property(&mut sink, string_named_descriptor, b"n\0a\0m\0e\0d\0\0\0")?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 0,
            index: 3,
            attachment_type: Some(i32::from(b'i')),
            data_size: None,
            filename: Some("embedded.msg".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 0,
            index: 3,
        })?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 0,
            index: 4,
            attachment_type: Some(i32::from(b'i')),
            data_size: None,
            filename: Some("second.msg".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 0,
            index: 4,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 0,
            complete: true,
        })?;

        message_start(&mut sink, 0, Some(0), Some((0, 3)))?;
        property(
            &mut sink,
            PropertyDescriptor {
                owner: PropertyOwner::Message(0),
                record_set_index: 0,
                entry_index: 0,
                entry_type: Some(0x1000),
                value_type: Some(0x001f),
                data_size: 4,
            },
            b"B\0D\0",
        )?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 0,
            index: 9,
            attachment_type: Some(i32::from(b'i')),
            data_size: None,
            filename: Some("nested.msg".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 0,
            index: 9,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 0,
            complete: true,
        })?;

        sink.event(CatalogEvent::MessageStart {
            id: 0,
            provenance: CatalogProvenance::Normal,
            recovery_index: None,
            folder_id: Some(0),
            parent_message_id: Some(0),
            parent_attachment_index: Some(9),
            embedded_path: vec![3, 9],
            associated: false,
            item_type: Some(11),
            message_class: Some("IPM.Note".to_owned()),
            subject: Some("nested embedded".to_owned()),
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported: true,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 0,
            complete: true,
        })?;

        message_start(&mut sink, 0, Some(0), Some((0, 4)))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 0,
            complete: true,
        })?;
        sink.event(CatalogEvent::UnitEnd(message_unit))?;

        let mail = load_canonical_mail(&sink)?;
        assert_eq!(mail.len(), 1);
        assert_eq!(mail[0].folder_path, ["Inbox"]);
        assert_eq!(mail[0].recipients[0].properties.len(), 1);
        assert_eq!(mail[0].attachments[0].index, 3);
        assert_eq!(
            mail[0].attachments[0]
                .embedded
                .as_ref()
                .map(|message| message.durable_item_key.as_str()),
            Some("normal:-:-:1")
        );
        assert_eq!(
            mail[0].attachments[1]
                .embedded
                .as_ref()
                .map(|message| message.durable_item_key.as_str()),
            Some("normal:-:-:3")
        );
        assert_eq!(mail[0].spooled_bytes, 25);
        let input = build_part_writer_input(
            &sink,
            &[&mail[0]],
            &"0".repeat(64),
            "balanced",
            8 * 1024 * 1024,
            1,
        )?;
        assert_eq!(input.store.folders[0].path, ["Inbox"]);
        assert_eq!(input.store.folders[0].messages[0].attachments.len(), 2);
        assert_eq!(
            input.store.folders[0].messages[0].named_properties[0].name,
            pstforge_pst::writer::NamedPropertyName::Numeric(0x6204)
        );
        assert_eq!(
            input.store.folders[0].messages[0].named_properties[1].name,
            pstforge_pst::writer::NamedPropertyName::String("CheckpointName".to_owned())
        );
        let pstforge_pst::writer::AttachmentContent::Embedded(embedded) =
            &input.store.folders[0].messages[0].attachments[0].content
        else {
            return Err("first attachment is not embedded".into());
        };
        assert_eq!(embedded.spooled_properties[0].id, 0x1000);
        assert_eq!(embedded.spooled_properties[0].blob.byte_len, 4);
        assert_eq!(embedded.attachments.len(), 1);
        let pstforge_pst::writer::AttachmentContent::Embedded(nested) =
            &embedded.attachments[0].content
        else {
            return Err("nested attachment is not embedded".into());
        };
        assert_eq!(nested.subject, "nested embedded");
        assert_eq!(input.omitted_attachments, 0);
        assert!(input.unsupported_item_keys.is_empty());
        let output = directory.path().join("translated.pst");
        pstforge_pst::writer::create_mail_store(&output, &input.store)?;
        assert!(output.is_file());
        sink.checkpoint()?;
        let (parts, written, partial) = split_recovered_job(
            &directory.path().join("job"),
            &"0".repeat(64),
            RecoveryMode::Balanced,
            1,
        )?;
        assert_eq!(parts.len(), 1);
        assert!(parts[0].oversize);
        assert_eq!(written, 4);
        assert!(partial);
        assert!(directory.path().join("job/parts/part-0001.pst").is_file());
        assert!(
            directory
                .path()
                .join("job/.pstforge/manifests/part-0001.json")
                .is_file()
        );
        let reopened = DurableCatalogSink::open(&directory.path().join("job"))?;
        assert_eq!(reopened.summary()?.unsupported_candidates, 0);
        assert!(reopened.spooled_candidates()?.is_empty());
        drop(reopened);
        let (resumed_parts, resumed_written, resumed_partial) = split_recovered_job(
            &directory.path().join("job"),
            &"0".repeat(64),
            RecoveryMode::Balanced,
            1,
        )?;
        assert_eq!(resumed_parts, parts);
        assert_eq!(resumed_written, written);
        assert!(resumed_partial);
        Ok(())
    }

    #[test]
    fn supported_child_of_unsupported_parent_remains_writable()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let mut sink = DurableCatalogSink::create(&directory.path().join("job"))?;
        sink.event(CatalogEvent::MessageStart {
            id: 20,
            provenance: CatalogProvenance::Recovered,
            recovery_index: Some(7),
            folder_id: None,
            parent_message_id: None,
            parent_attachment_index: None,
            embedded_path: Vec::new(),
            associated: false,
            item_type: Some(5),
            message_class: Some("IPM.Task".to_owned()),
            subject: None,
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported: false,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 20,
            complete: true,
        })?;
        sink.event(CatalogEvent::MessageStart {
            id: 21,
            provenance: CatalogProvenance::Recovered,
            recovery_index: Some(7),
            folder_id: None,
            parent_message_id: Some(20),
            parent_attachment_index: Some(0),
            embedded_path: vec![0],
            associated: false,
            item_type: Some(11),
            message_class: Some("IPM.Note".to_owned()),
            subject: Some("writable child".to_owned()),
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported: true,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 21,
            complete: true,
        })?;

        let mail = load_canonical_mail(&sink)?;
        assert_eq!(mail.len(), 1);
        assert_eq!(mail[0].key.source_node_id, Some(21));
        assert_eq!(mail[0].folder_path, ["Recovered Mail"]);
        Ok(())
    }

    #[test]
    fn removes_only_pst_infrastructure_from_normal_folder_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let mut sink = DurableCatalogSink::create(&directory.path().join("job"))?;
        let root = FolderAddress::root();
        let ipm = root.child(0).ok_or("IPM address")?;
        let inbox = ipm.child(0).ok_or("Inbox address")?;
        for (address, id, parent_id, name) in [
            (root, 0x122, None, None),
            (
                ipm,
                0x8022,
                Some(0x122),
                Some("Localized data-file container"),
            ),
            (inbox, 0x8082, Some(0x8022), Some("Inbox")),
        ] {
            sink.event(CatalogEvent::UnitStart(RecoveryUnit::Folder { address }))?;
            sink.event(CatalogEvent::Folder {
                id,
                parent_id,
                name: name.map(str::to_owned),
                container_class: None,
            })?;
            sink.event(CatalogEvent::UnitEnd(RecoveryUnit::Folder { address }))?;
        }
        let message_unit = RecoveryUnit::Normal {
            folder: inbox,
            folder_id: 0x8082,
            message_index: 0,
        };
        sink.event(CatalogEvent::UnitStart(message_unit))?;
        message_start(&mut sink, 0x9004, Some(0x8062), None)?;
        sink.event(CatalogEvent::MessageEnd {
            id: 0x9004,
            complete: true,
        })?;
        sink.event(CatalogEvent::UnitEnd(message_unit))?;

        let mail = load_canonical_mail(&sink)?;
        assert_eq!(mail.len(), 1);
        assert_eq!(mail[0].folder_path, ["Inbox"]);
        assert_eq!(mail[0].folder_role, CanonicalFolderRole::Ordinary);
        Ok(())
    }

    #[test]
    fn reconstructs_empty_ipm_folders_by_catalog_address_despite_duplicate_source_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let mut sink = DurableCatalogSink::create(&directory.path().join("job"))?;
        let root = FolderAddress::root();
        let ipm = root.child(0).ok_or("IPM address")?;
        let inbox = ipm.child(0).ok_or("Inbox address")?;
        let deleted = ipm.child(1).ok_or("Deleted Items address")?;
        let user_deleted = ipm.child(2).ok_or("user folder address")?;
        let duplicate_deleted = ipm.child(3).ok_or("duplicate Deleted Items address")?;
        let empty_child = user_deleted.child(0).ok_or("empty child address")?;
        let search_root = root.child(1).ok_or("search root address")?;
        for (address, id, parent_id, name) in [
            (root, 0x122, None, None),
            (ipm, 0x8022, Some(0x122), Some("Top of Outlook data file")),
            (inbox, 0x8082, Some(0x8022), Some("Inbox")),
            (deleted, 0x8062, Some(0x8022), Some("Deleted Items")),
            (user_deleted, 0x8022, Some(0x8022), Some("Deleted items")),
            (
                duplicate_deleted,
                0x8062,
                Some(0x8022),
                Some("Other Deleted"),
            ),
            (empty_child, 0x9003, Some(0x8022), Some("Empty Child")),
            (search_root, 0x8023, Some(0x122), Some("Search Root")),
        ] {
            sink.event(CatalogEvent::UnitStart(RecoveryUnit::Folder { address }))?;
            sink.event(CatalogEvent::Folder {
                id,
                parent_id,
                name: name.map(str::to_owned),
                container_class: (name == Some("Empty Child")).then(|| "IPF.Contact".to_owned()),
            })?;
            sink.event(CatalogEvent::UnitEnd(RecoveryUnit::Folder { address }))?;
        }

        let folder_set = load_canonical_folders(&sink)?;
        assert_eq!(folder_set.omitted_folders, 0);
        assert_eq!(
            folder_set.folders,
            [
                CanonicalFolder {
                    path: vec!["Deleted Items".to_owned()],
                    location: CanonicalFolderLocation::IpmSubtree,
                    role: CanonicalFolderRole::DeletedItems,
                    container_class: None,
                },
                CanonicalFolder {
                    path: vec!["Deleted items".to_owned()],
                    location: CanonicalFolderLocation::IpmSubtree,
                    role: CanonicalFolderRole::Ordinary,
                    container_class: None,
                },
                CanonicalFolder {
                    path: vec!["Deleted items".to_owned(), "Empty Child".to_owned()],
                    location: CanonicalFolderLocation::IpmSubtree,
                    role: CanonicalFolderRole::Ordinary,
                    container_class: Some("IPF.Contact".to_owned()),
                },
                CanonicalFolder {
                    path: vec!["Inbox".to_owned()],
                    location: CanonicalFolderLocation::IpmSubtree,
                    role: CanonicalFolderRole::Ordinary,
                    container_class: None,
                },
                CanonicalFolder {
                    path: vec!["Other Deleted".to_owned()],
                    location: CanonicalFolderLocation::IpmSubtree,
                    role: CanonicalFolderRole::Ordinary,
                    container_class: None,
                },
            ]
        );
        let message_unit = RecoveryUnit::Normal {
            folder: inbox,
            folder_id: 0x8082,
            message_index: 0,
        };
        sink.event(CatalogEvent::UnitStart(message_unit))?;
        message_start(&mut sink, 0x9104, Some(0x8082), None)?;
        sink.event(CatalogEvent::MessageEnd {
            id: 0x9104,
            complete: true,
        })?;
        sink.event(CatalogEvent::UnitEnd(message_unit))?;
        sink.checkpoint()?;
        let (parts, written, _partial) = split_recovered_job(
            &directory.path().join("job"),
            &"1".repeat(64),
            RecoveryMode::Balanced,
            4_294_967_296,
        )?;
        assert_eq!(parts.len(), 1);
        assert_eq!(written, 1);
        let output = directory.path().join("job/parts/part-0001.pst");
        let verified = crate::verify(&output)?;
        assert_eq!(verified.inventory.folders, 9);
        assert_eq!(verified.inventory.normal_items, 1);
        Ok(())
    }

    #[test]
    fn reports_duplicate_visible_folder_paths_instead_of_hiding_loss()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let mut sink = DurableCatalogSink::create(&directory.path().join("job"))?;
        let root = FolderAddress::root();
        let ipm = root.child(0).ok_or("IPM address")?;
        let first = ipm.child(0).ok_or("first folder address")?;
        let duplicate = ipm.child(1).ok_or("duplicate folder address")?;
        for (address, id, parent_id, name) in [
            (root, 0x122, None, None),
            (ipm, 0x8022, Some(0x122), Some("Top of Outlook data file")),
            (first, 0x9001, Some(0x8022), Some("Duplicate")),
            (duplicate, 0x9002, Some(0x8022), Some("Duplicate")),
        ] {
            sink.event(CatalogEvent::UnitStart(RecoveryUnit::Folder { address }))?;
            sink.event(CatalogEvent::Folder {
                id,
                parent_id,
                name: name.map(str::to_owned),
                container_class: None,
            })?;
            sink.event(CatalogEvent::UnitEnd(RecoveryUnit::Folder { address }))?;
        }

        let folder_set = load_canonical_folders(&sink)?;
        assert_eq!(folder_set.omitted_folders, 1);
        assert_eq!(
            folder_set.folders,
            [CanonicalFolder {
                path: vec!["Duplicate".to_owned()],
                location: CanonicalFolderLocation::IpmSubtree,
                role: CanonicalFolderRole::Ordinary,
                container_class: None,
            }]
        );
        Ok(())
    }

    #[test]
    fn preserves_store_root_and_associated_placement_as_typed_source_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let mut sink = DurableCatalogSink::create(&directory.path().join("job"))?;
        let root = FolderAddress::root();
        let freebusy = root.child(0).ok_or("Freebusy address")?;
        for (address, id, parent_id, name) in [
            (root, 0x122, None, None),
            (freebusy, 0x82C2, Some(0x122), Some("Freebusy Data")),
        ] {
            sink.event(CatalogEvent::UnitStart(RecoveryUnit::Folder { address }))?;
            sink.event(CatalogEvent::Folder {
                id,
                parent_id,
                name: name.map(str::to_owned),
                container_class: None,
            })?;
            sink.event(CatalogEvent::UnitEnd(RecoveryUnit::Folder { address }))?;
        }

        let unit = RecoveryUnit::Associated {
            folder: freebusy,
            folder_id: 0x82C2,
            message_index: 0,
        };
        sink.event(CatalogEvent::UnitStart(unit))?;
        sink.event(CatalogEvent::MessageStart {
            id: 0x9008,
            provenance: CatalogProvenance::Normal,
            recovery_index: None,
            folder_id: Some(0x82C2),
            parent_message_id: None,
            parent_attachment_index: None,
            embedded_path: Vec::new(),
            associated: true,
            item_type: Some(6),
            message_class: Some("IPM.Configuration.PSTForge".to_owned()),
            subject: Some("associated checkpoint".to_owned()),
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported: true,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 0x9008,
            complete: true,
        })?;
        sink.event(CatalogEvent::UnitEnd(unit))?;

        let folders = load_canonical_folders(&sink)?;
        assert_eq!(
            folders.folders,
            [CanonicalFolder {
                path: vec!["Freebusy Data".to_owned()],
                location: CanonicalFolderLocation::StoreRoot,
                role: CanonicalFolderRole::Ordinary,
                container_class: None,
            }]
        );
        let messages = load_canonical_mail(&sink)?;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].folder_path, ["Freebusy Data"]);
        assert_eq!(
            messages[0].folder_location,
            CanonicalFolderLocation::StoreRoot
        );
        assert_eq!(
            messages[0].placement,
            super::CanonicalMessagePlacement::Associated
        );
        Ok(())
    }

    #[test]
    fn omits_invalid_source_only_folder_without_suppressing_valid_mail()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let mut sink = DurableCatalogSink::create(&directory.path().join("job"))?;
        let root = FolderAddress::root();
        let ipm = root.child(0).ok_or("IPM address")?;
        let inbox = ipm.child(0).ok_or("Inbox address")?;
        let invalid = ipm.child(1).ok_or("invalid folder address")?;
        let invalid_name = "x".repeat(2049);
        for (address, id, parent_id, name) in [
            (root, 0x122, None, None),
            (ipm, 0x8022, Some(0x122), Some("Top of Outlook data file")),
            (inbox, 0x8082, Some(0x8022), Some("Inbox")),
            (invalid, 0x9001, Some(0x8022), Some(invalid_name.as_str())),
        ] {
            sink.event(CatalogEvent::UnitStart(RecoveryUnit::Folder { address }))?;
            sink.event(CatalogEvent::Folder {
                id,
                parent_id,
                name: name.map(str::to_owned),
                container_class: None,
            })?;
            sink.event(CatalogEvent::UnitEnd(RecoveryUnit::Folder { address }))?;
        }
        let message_unit = RecoveryUnit::Normal {
            folder: inbox,
            folder_id: 0x8082,
            message_index: 0,
        };
        sink.event(CatalogEvent::UnitStart(message_unit))?;
        message_start(&mut sink, 0x9104, Some(0x8082), None)?;
        sink.event(CatalogEvent::MessageEnd {
            id: 0x9104,
            complete: true,
        })?;
        sink.event(CatalogEvent::UnitEnd(message_unit))?;
        sink.checkpoint()?;

        let (parts, written, partial) = split_recovered_job(
            &directory.path().join("job"),
            &"1".repeat(64),
            RecoveryMode::Balanced,
            4_294_967_296,
        )?;
        assert_eq!(parts.len(), 1);
        assert_eq!(written, 1);
        assert!(partial);
        assert_eq!(parts[0].message_count, 1);
        assert_eq!(parts[0].omitted_folders, 1);
        Ok(())
    }

    #[test]
    fn unsupported_embedded_child_is_omitted_without_losing_parent()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 30, None, None)?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 30,
            index: 2,
            attachment_type: Some(i32::from(b'i')),
            data_size: None,
            filename: Some("unsupported-contact.msg".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 30,
            index: 2,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 30,
            complete: true,
        })?;
        sink.event(CatalogEvent::MessageStart {
            id: 31,
            provenance: CatalogProvenance::Normal,
            recovery_index: None,
            folder_id: None,
            parent_message_id: Some(30),
            parent_attachment_index: Some(2),
            embedded_path: vec![2],
            associated: false,
            item_type: Some(5),
            message_class: Some("IPM.Task".to_owned()),
            subject: Some("unsupported embedded task".to_owned()),
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported: false,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 31,
            complete: true,
        })?;

        let mail = load_canonical_mail(&sink)?;
        assert_eq!(mail.len(), 1);
        assert_eq!(mail[0].key.source_node_id, Some(30));
        assert_eq!(mail[0].attachments.len(), 1);
        assert!(mail[0].attachments[0].embedded.is_none());
        let input = build_part_writer_input(
            &sink,
            &[&mail[0]],
            &"0".repeat(64),
            "balanced",
            8 * 1024 * 1024,
            1,
        )?;
        assert_eq!(input.omitted_attachments, 1);

        sink.checkpoint()?;
        drop(sink);
        let (parts, written, partial) = split_recovered_job(
            &job,
            &"0".repeat(64),
            RecoveryMode::Balanced,
            8 * 1024 * 1024,
        )?;
        assert_eq!(parts.len(), 1);
        assert_eq!(written, 1);
        assert!(partial);
        assert!(job.join("parts/part-0001.pst").is_file());
        Ok(())
    }

    #[test]
    fn rejects_reparented_embedded_ownership_and_property_owners()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("ownership-job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 40, None, None)?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 40,
            index: 2,
            attachment_type: Some(i32::from(b'i')),
            data_size: None,
            filename: Some("embedded-40.msg".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 40,
            index: 2,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 40,
            complete: true,
        })?;
        message_start(&mut sink, 41, None, Some((40, 2)))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 41,
            complete: true,
        })?;
        message_start(&mut sink, 50, None, None)?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 50,
            index: 3,
            attachment_type: Some(i32::from(b'i')),
            data_size: None,
            filename: Some("embedded-50.msg".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 50,
            index: 3,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 50,
            complete: true,
        })?;
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        connection.execute(
            "UPDATE candidates SET parent_item_key = ?1, parent_attachment_index = 3 \
             WHERE item_key = ?2",
            params!["normal:50:-:0", "normal:41:-:0"],
        )?;
        drop(connection);
        let reopened = DurableCatalogSink::open(&job)?;
        let error = load_canonical_mail(&reopened).expect_err("reparented child must be rejected");
        assert!(error.to_string().contains("ownership columns disagree"));

        let property_job = directory.path().join("property-job");
        let mut sink = DurableCatalogSink::create(&property_job)?;
        message_start(&mut sink, 60, None, None)?;
        property(
            &mut sink,
            PropertyDescriptor {
                owner: PropertyOwner::Message(60),
                record_set_index: 0,
                entry_index: 0,
                entry_type: Some(0x1000),
                value_type: Some(0x001f),
                data_size: 2,
            },
            b"X\0",
        )?;
        sink.event(CatalogEvent::MessageEnd {
            id: 60,
            complete: true,
        })?;
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(property_job.join(".pstforge/job.sqlite3"))?;
        let metadata: String = connection.query_row(
            "SELECT metadata_json FROM candidate_events WHERE kind = 'property'",
            [],
            |row| row.get(0),
        )?;
        let mut metadata: serde_json::Value = serde_json::from_str(&metadata)?;
        metadata["owner_id"] = serde_json::json!(61);
        connection.execute(
            "UPDATE candidate_events SET metadata_json = ?1 WHERE kind = 'property'",
            [serde_json::to_string(&metadata)?],
        )?;
        drop(connection);
        let reopened = DurableCatalogSink::open(&property_job)?;
        let error = load_canonical_mail(&reopened).expect_err("foreign property owner must fail");
        assert!(error.to_string().contains("owner id disagrees"));

        let terminal_job = directory.path().join("terminal-job");
        let mut sink = DurableCatalogSink::create(&terminal_job)?;
        message_start(&mut sink, 80, None, None)?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 80,
            index: 0,
            attachment_type: Some(i32::from(b'd')),
            data_size: Some(0),
            filename: Some("empty.bin".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 80,
            index: 0,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 80,
            complete: true,
        })?;
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(terminal_job.join(".pstforge/job.sqlite3"))?;
        let metadata: String = connection.query_row(
            "SELECT metadata_json FROM candidate_events WHERE kind = 'attachment_data'",
            [],
            |row| row.get(0),
        )?;
        let mut metadata: serde_json::Value = serde_json::from_str(&metadata)?;
        metadata["message_id"] = serde_json::json!(81);
        connection.execute(
            "UPDATE candidate_events SET metadata_json = ?1 WHERE kind = 'attachment_data'",
            [serde_json::to_string(&metadata)?],
        )?;
        drop(connection);
        let reopened = DurableCatalogSink::open(&terminal_job)?;
        let error = load_canonical_mail(&reopened).expect_err("foreign terminal owner must fail");
        assert!(error.to_string().contains("event message id disagrees"));

        let incomplete_job = directory.path().join("incomplete-property-job");
        let mut sink = DurableCatalogSink::create(&incomplete_job)?;
        message_start(&mut sink, 90, None, None)?;
        sink.event(CatalogEvent::Recipient {
            message_id: 90,
            index: 0,
            recipient_type: Some(1),
            display_name: None,
            email_address: None,
            address_type: None,
        })?;
        let descriptor = PropertyDescriptor {
            owner: PropertyOwner::Recipient {
                message_id: 90,
                index: 0,
            },
            record_set_index: 0,
            entry_index: 0,
            entry_type: Some(0x3001),
            value_type: Some(0x001f),
            data_size: 4,
        };
        sink.event(CatalogEvent::PropertyStart(descriptor))?;
        sink.event(CatalogEvent::PropertyAbort {
            descriptor,
            reason: "damaged".to_owned(),
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 90,
            complete: false,
        })?;
        sink.checkpoint()?;
        drop(sink);
        let connection = Connection::open(incomplete_job.join(".pstforge/job.sqlite3"))?;
        let metadata: String = connection.query_row(
            "SELECT metadata_json FROM candidate_events WHERE kind = 'property_incomplete'",
            [],
            |row| row.get(0),
        )?;
        let mut metadata: serde_json::Value = serde_json::from_str(&metadata)?;
        metadata["property"]["owner_index"] = serde_json::json!(1);
        connection.execute(
            "UPDATE candidate_events SET metadata_json = ?1 WHERE kind = 'property_incomplete'",
            [serde_json::to_string(&metadata)?],
        )?;
        drop(connection);
        let reopened = DurableCatalogSink::open(&incomplete_job)?;
        let error = load_canonical_mail(&reopened)
            .expect_err("unknown incomplete property owner must fail");
        assert!(error.to_string().contains("owns unknown recipient"));
        Ok(())
    }

    #[test]
    fn rejects_overdepth_durable_ownership_before_recursive_rebuild()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("overdepth-job");
        let mut sink = DurableCatalogSink::create(&job)?;
        message_start(&mut sink, 80, None, None)?;
        sink.event(CatalogEvent::AttachmentStart {
            message_id: 80,
            index: 1,
            attachment_type: Some(i32::from(b'i')),
            data_size: None,
            filename: Some("embedded.msg".to_owned()),
        })?;
        sink.event(CatalogEvent::AttachmentEnd {
            message_id: 80,
            index: 1,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 80,
            complete: true,
        })?;
        message_start(&mut sink, 81, None, Some((80, 1)))?;
        sink.event(CatalogEvent::MessageEnd {
            id: 81,
            complete: true,
        })?;
        sink.checkpoint()?;
        drop(sink);

        let connection = Connection::open(job.join(".pstforge/job.sqlite3"))?;
        let item_key = "normal:81:-:0";
        let metadata_json: String = connection.query_row(
            "SELECT metadata_json FROM candidates WHERE item_key = ?1",
            [item_key],
            |row| row.get(0),
        )?;
        let mut metadata: serde_json::Value = serde_json::from_str(&metadata_json)?;
        let embedded_path = vec![1_u32; 257];
        metadata["embedded_path"] = serde_json::to_value(&embedded_path)?;
        connection.execute(
            "UPDATE candidates SET embedded_path_json = ?1, metadata_json = ?2 \
             WHERE item_key = ?3",
            params![
                serde_json::to_string(&embedded_path)?,
                serde_json::to_string(&metadata)?,
                item_key
            ],
        )?;
        drop(connection);

        let reopened = DurableCatalogSink::open(&job)?;
        let error =
            load_canonical_mail(&reopened).expect_err("overdepth durable state must be rejected");
        assert!(error.to_string().contains("supported depth of 256"));
        Ok(())
    }

    #[test]
    fn duplicate_children_of_unsupported_parent_are_rejected_globally()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let mut sink = DurableCatalogSink::create(&directory.path().join("job"))?;
        sink.event(CatalogEvent::MessageStart {
            id: 70,
            provenance: CatalogProvenance::Recovered,
            recovery_index: Some(4),
            folder_id: None,
            parent_message_id: None,
            parent_attachment_index: None,
            embedded_path: Vec::new(),
            associated: false,
            item_type: Some(5),
            message_class: Some("IPM.Task".to_owned()),
            subject: None,
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            supported: false,
        })?;
        sink.event(CatalogEvent::MessageEnd {
            id: 70,
            complete: true,
        })?;
        for id in [71, 72] {
            sink.event(CatalogEvent::MessageStart {
                id,
                provenance: CatalogProvenance::Recovered,
                recovery_index: Some(4),
                folder_id: None,
                parent_message_id: Some(70),
                parent_attachment_index: Some(1),
                embedded_path: vec![1],
                associated: false,
                item_type: Some(11),
                message_class: Some("IPM.Note".to_owned()),
                subject: Some(format!("child {id}")),
                sender_name: None,
                sender_email: None,
                submit_filetime: None,
                delivery_filetime: None,
                supported: true,
            })?;
            sink.event(CatalogEvent::MessageEnd { id, complete: true })?;
        }
        let error = load_canonical_mail(&sink).expect_err("duplicate slot claims must fail");
        assert!(error.to_string().contains("also claimed"));
        Ok(())
    }

    #[test]
    fn split_contains_unrepresentable_candidate_and_writes_later_mail()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let mut sink = DurableCatalogSink::create(&job)?;
        for (id, subject) in [(1, "x".repeat(2049)), (2, "valid".to_owned())] {
            sink.event(CatalogEvent::MessageStart {
                id,
                provenance: CatalogProvenance::Normal,
                recovery_index: None,
                folder_id: None,
                parent_message_id: None,
                parent_attachment_index: None,
                embedded_path: Vec::new(),
                associated: false,
                item_type: Some(11),
                message_class: Some("IPM.Note".to_owned()),
                subject: Some(subject),
                sender_name: Some("Sender".to_owned()),
                sender_email: Some("sender@example.com".to_owned()),
                submit_filetime: None,
                delivery_filetime: None,
                supported: true,
            })?;
            sink.event(CatalogEvent::MessageEnd { id, complete: true })?;
        }
        sink.checkpoint()?;
        drop(sink);

        let (parts, written, partial) = split_recovered_job(
            &job,
            &"0".repeat(64),
            RecoveryMode::Balanced,
            8 * 1024 * 1024,
        )?;
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].message_count, 1);
        assert_eq!(written, 1);
        assert!(partial);

        let reopened = DurableCatalogSink::open(&job)?;
        let summary = reopened.summary()?;
        assert_eq!(summary.committed_candidates, 2);
        assert_eq!(summary.unsupported_candidates, 1);
        assert_eq!(reopened.spooled_candidates()?.len(), 0);
        Ok(())
    }
}
