#![deny(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use libpff_sys::{CatalogEvent, CatalogSink, PropertyOwner};
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
                            0x007d | 0x0e07 | 0x1000 | 0x1009 | 0x1013 | 0x3007 | 0x3008 | 0x3fde
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
