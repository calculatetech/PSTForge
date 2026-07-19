use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

use pstforge_job::{
    DurableCatalogSink, JobError, ReconstructedField, ReconstructionCounts, SpooledBlob,
};
use pstforge_pst::writer::{
    AttachmentContent, AttachmentSpec, FileBlobSpec, MailFolderLocation, MailFolderRole,
    MailFolderSpec, MailStoreSpec, MessageSpec, NamedProperty, NamedPropertyName, NamedPropertySet,
    NativeBody, RawProperty, RawPropertyValue, RecipientKind, RecipientSpec, SpooledPropertySpec,
    UnsupportedProperty,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    CanonicalAttachment, CanonicalFolder, CanonicalFolderLocation, CanonicalFolderRole,
    CanonicalMail, CanonicalMessagePlacement, CanonicalProperty, CanonicalRecipient,
    attachment_mime::{self, BlobRange},
};

const PSETID_ADDRESS: [u8; 16] = [
    0x04, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];
const MAX_DISTRIBUTION_LIST_PROPERTY_BYTES: u64 = 15_000;

#[derive(Debug, Error)]
pub enum CanonicalWriteError {
    #[error(transparent)]
    Job(#[from] JobError),
    #[error("candidate {item_key} cannot be translated: {detail}")]
    InvalidCandidate { item_key: String, detail: String },
    #[error("candidate {item_key} property 0x{property_id:04X} is malformed: {detail}")]
    InvalidProperty {
        item_key: String,
        property_id: u32,
        detail: String,
    },
    #[error("cannot read a verified spool blob for candidate {item_key}: {source}")]
    BlobRead {
        item_key: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartWriterInput {
    pub store: MailStoreSpec,
    pub item_keys: Vec<String>,
    pub unsupported_item_keys: Vec<String>,
    pub partial: bool,
    pub omitted_folders: u64,
    pub omitted_properties: u64,
    pub omitted_attachments: u64,
    pub reconstructions: ReconstructionCounts,
}

pub(crate) struct PartBuildOptions<'a> {
    pub source_sha256: &'a str,
    pub recovery_mode: &'a str,
    pub maximum_pst_bytes: u64,
    pub part_index: u32,
    pub omitted_folders: u64,
}

pub fn build_part_writer_input(
    job: &DurableCatalogSink,
    messages: &[&CanonicalMail],
    source_sha256: &str,
    recovery_mode: &str,
    maximum_pst_bytes: u64,
    part_index: u32,
) -> Result<PartWriterInput, CanonicalWriteError> {
    build_part_writer_input_expected(
        job,
        messages,
        &[],
        PartBuildOptions {
            source_sha256,
            recovery_mode,
            maximum_pst_bytes,
            part_index,
            omitted_folders: 0,
        },
        None,
    )
}

pub fn build_part_writer_input_interruptible(
    job: &DurableCatalogSink,
    messages: &[&CanonicalMail],
    source_sha256: &str,
    recovery_mode: &str,
    maximum_pst_bytes: u64,
    part_index: u32,
    interrupted: &AtomicBool,
) -> Result<PartWriterInput, CanonicalWriteError> {
    build_part_writer_input_expected(
        job,
        messages,
        &[],
        PartBuildOptions {
            source_sha256,
            recovery_mode,
            maximum_pst_bytes,
            part_index,
            omitted_folders: 0,
        },
        Some(interrupted),
    )
}

pub(crate) fn build_part_writer_input_with_folders_interruptible(
    job: &DurableCatalogSink,
    messages: &[&CanonicalMail],
    source_folders: &[CanonicalFolder],
    options: PartBuildOptions<'_>,
    interrupted: &AtomicBool,
) -> Result<PartWriterInput, CanonicalWriteError> {
    build_part_writer_input_expected(job, messages, source_folders, options, Some(interrupted))
}

fn build_part_writer_input_expected(
    job: &DurableCatalogSink,
    messages: &[&CanonicalMail],
    source_folders: &[CanonicalFolder],
    options: PartBuildOptions<'_>,
    interrupted: Option<&AtomicBool>,
) -> Result<PartWriterInput, CanonicalWriteError> {
    let PartBuildOptions {
        source_sha256,
        recovery_mode,
        maximum_pst_bytes,
        part_index,
        omitted_folders,
    } = options;
    let source = TranslationSource { job, interrupted };
    source.check_interrupted()?;
    if messages.is_empty() {
        return Err(CanonicalWriteError::InvalidCandidate {
            item_key: "<part>".to_owned(),
            detail: "part has no messages".to_owned(),
        });
    }
    let mut reconstructions = ReconstructionCounts::default();
    let mut folders = source_folders
        .iter()
        .map(|folder| {
            let container_class = folder
                .container_class
                .clone()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    reconstructions.record_generated(ReconstructedField::FolderClass);
                    "IPF.Note".to_owned()
                });
            (
                (folder.location, folder.path.clone()),
                (folder.role, container_class, Vec::new(), Vec::new()),
            )
        })
        .collect::<BTreeMap<
            (CanonicalFolderLocation, Vec<String>),
            (
                CanonicalFolderRole,
                String,
                Vec<MessageSpec>,
                Vec<MessageSpec>,
            ),
        >>();
    let mut item_keys = Vec::with_capacity(messages.len());
    let mut unsupported_item_keys = Vec::new();
    let mut partial = false;
    let mut omitted_properties = 0_u64;
    let mut omitted_attachments = 0_u64;
    for mail in messages {
        source.check_interrupted()?;
        let mut translated = translate_message(&source, mail, false)?;
        partial |= translated.partial;
        omitted_properties = omitted_properties.saturating_add(translated.omitted_properties);
        omitted_attachments = omitted_attachments.saturating_add(translated.omitted_attachments);
        if mail.placement == CanonicalMessagePlacement::Associated {
            normalize_associated_display_name(&mut translated.message, &mut reconstructions);
        }
        reconstructions.merge(translated.reconstructions);
        match folders.entry((mail.folder_location, mail.folder_path.clone())) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                let mut normal = Vec::new();
                let mut associated = Vec::new();
                match mail.placement {
                    CanonicalMessagePlacement::Normal => normal.push(translated.message),
                    CanonicalMessagePlacement::Associated => associated.push(translated.message),
                }
                let container_class =
                    default_container_class(mail.message_class.as_deref().unwrap_or("IPM.Note"))
                        .to_owned();
                if mail
                    .message_class
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
                {
                    reconstructions.record_derived(ReconstructedField::FolderClass);
                } else {
                    reconstructions.record_generated(ReconstructedField::FolderClass);
                }
                entry.insert((mail.folder_role, container_class, normal, associated));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let (role, _, messages, associated_messages) = entry.get_mut();
                if mail.folder_role == CanonicalFolderRole::DeletedItems {
                    *role = CanonicalFolderRole::DeletedItems;
                }
                match mail.placement {
                    CanonicalMessagePlacement::Normal => messages.push(translated.message),
                    CanonicalMessagePlacement::Associated => {
                        associated_messages.push(translated.message);
                    }
                }
            }
        }
        item_keys.extend(translated.item_keys);
        unsupported_item_keys.extend(translated.unsupported_item_keys);
    }
    let folders = folders
        .into_iter()
        .map(
            |((location, path), (role, container_class, messages, associated_messages))| {
                MailFolderSpec {
                    path,
                    location: match location {
                        CanonicalFolderLocation::StoreRoot => MailFolderLocation::StoreRoot,
                        CanonicalFolderLocation::IpmSubtree => MailFolderLocation::IpmSubtree,
                    },
                    role: match role {
                        CanonicalFolderRole::Ordinary => MailFolderRole::Ordinary,
                        CanonicalFolderRole::DeletedItems => MailFolderRole::DeletedItems,
                    },
                    container_class,
                    messages,
                    associated_messages,
                }
            },
        )
        .collect();
    unsupported_item_keys.sort();
    unsupported_item_keys.dedup();
    Ok(PartWriterInput {
        store: MailStoreSpec {
            store_name: format!("PSTForge Recovery Part {part_index:04}"),
            record_key: part_record_key(
                source_sha256,
                recovery_mode,
                maximum_pst_bytes,
                part_index,
            )?,
            folders,
        },
        item_keys,
        unsupported_item_keys,
        partial: partial || omitted_folders != 0,
        omitted_folders,
        omitted_properties,
        omitted_attachments,
        reconstructions,
    })
}

fn normalize_associated_display_name(
    message: &mut MessageSpec,
    reconstructions: &mut ReconstructionCounts,
) {
    let display_name = message
        .raw_properties
        .iter_mut()
        .find_map(|property| match property {
            RawProperty {
                id: 0x3001,
                value: RawPropertyValue::Unicode(value),
            } => Some(value),
            _ => None,
        });
    match display_name {
        Some(value) if value.is_empty() && message.subject.is_empty() => {
            *value = "(no subject)".to_owned();
            reconstructions.record_generated(ReconstructedField::AssociatedDisplayName);
        }
        Some(value) if value.is_empty() => {
            *value = message.subject.clone();
            reconstructions.record_derived(ReconstructedField::AssociatedDisplayName);
        }
        Some(_) => {}
        None if message.subject.is_empty() => {
            message.raw_properties.push(RawProperty {
                id: 0x3001,
                value: RawPropertyValue::Unicode("(no subject)".to_owned()),
            });
            reconstructions.record_generated(ReconstructedField::AssociatedDisplayName);
        }
        None => {
            reconstructions.record_derived(ReconstructedField::AssociatedDisplayName);
        }
    }
}

struct TranslationSource<'a> {
    job: &'a DurableCatalogSink,
    interrupted: Option<&'a AtomicBool>,
}

impl TranslationSource<'_> {
    fn check_interrupted(&self) -> Result<(), CanonicalWriteError> {
        if self
            .interrupted
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            Err(JobError::Interrupted.into())
        } else {
            Ok(())
        }
    }

    fn open_blob(&self, blob: &SpooledBlob) -> Result<std::fs::File, JobError> {
        match self.interrupted {
            Some(flag) => self.job.open_blob_interruptible(blob, flag),
            None => self.job.open_blob(blob),
        }
    }

    fn verified_blob_path(&self, blob: &SpooledBlob) -> Result<std::path::PathBuf, JobError> {
        match self.interrupted {
            Some(flag) => self.job.verified_blob_path_interruptible(blob, flag),
            None => self.job.verified_blob_path(blob),
        }
    }
}

struct TranslatedMessage {
    message: MessageSpec,
    item_keys: Vec<String>,
    unsupported_item_keys: Vec<String>,
    partial: bool,
    omitted_properties: u64,
    omitted_attachments: u64,
    reconstructions: ReconstructionCounts,
}

fn translate_message(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    allow_calendar_exception: bool,
) -> Result<TranslatedMessage, CanonicalWriteError> {
    let mut partial = !matches!(mail.completeness, crate::ContentCompleteness::Complete);
    let mut omitted_properties = 0_u64;
    let mut omitted_attachments = 0_u64;
    let mut raw_properties = Vec::new();
    let mut named_properties = Vec::new();
    let mut spooled_properties = Vec::new();
    let mut unsupported_properties = Vec::new();
    let mut native_body = None;
    let mut rtf_in_sync = false;
    let mut rtf_in_sync_observed = false;
    let mut creation_filetime = None;
    let mut modification_filetime = None;
    let mut message_flags = None;
    let mut internet_codepage = None;
    let mut html_property = None;
    let mut serialized_ids = BTreeSet::new();
    let mut serialized_named = BTreeSet::new();
    let mut item_keys = vec![mail.durable_item_key.clone()];
    let mut unsupported_item_keys = Vec::new();
    let mut reconstructions = ReconstructionCounts::default();

    for property in &mail.properties {
        let Some(property_type) = property
            .value_type
            .and_then(|value| u16::try_from(value).ok())
        else {
            partial = true;
            omitted_properties = omitted_properties.saturating_add(1);
            continue;
        };
        if let Some(identity) = &property.named_property {
            let transient_id = property
                .property_id
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(0x8000);
            let set = named_property_set(identity.guid);
            let name = match &identity.name {
                libpff_sys::NamedPropertyName::Numeric(value) => NamedPropertyName::Numeric(*value),
                libpff_sys::NamedPropertyName::String(value) => {
                    NamedPropertyName::String(value.clone())
                }
            };
            if !serialized_named.insert((set, name.clone())) {
                partial = true;
                omitted_properties = omitted_properties.saturating_add(1);
                continue;
            }
            let distribution_list_lid = match (&set, &name) {
                (NamedPropertySet::Guid(guid), NamedPropertyName::Numeric(lid))
                    if *guid == PSETID_ADDRESS =>
                {
                    Some(*lid)
                }
                _ => None,
            };
            let is_distribution_list_members =
                matches!(distribution_list_lid, Some(0x8054 | 0x8055));
            let is_distribution_list_checksum = distribution_list_lid == Some(0x804C);
            let is_public_keywords = set == NamedPropertySet::PublicStrings
                && name == NamedPropertyName::String("Keywords".to_owned());
            let omissions_before_value = omitted_properties;
            let mut value = if (is_distribution_list_members && property_type != 0x1102)
                || (is_distribution_list_checksum && property_type != 0x0003)
            {
                None
            } else {
                match property_type {
                    0x001F => read_unicode(job, mail, property)?
                        .map(RawPropertyValue::Unicode)
                        .or_else(|| Some(RawPropertyValue::Unicode(String::new()))),
                    0x0102 if property.blob.byte_len <= 1_048_576 => Some(
                        RawPropertyValue::Binary(read_blob(job, mail, property, 1_048_576)?),
                    ),
                    0x101F => omit_malformed(
                        multiple_unicode_property(job, mail, property, transient_id, 16 * 1024),
                        &mut omitted_properties,
                    )?,
                    0x1102 => omit_malformed(
                        multiple_binary_property(
                            job,
                            mail,
                            property,
                            transient_id,
                            if is_distribution_list_members {
                                MAX_DISTRIBUTION_LIST_PROPERTY_BYTES - 1
                            } else {
                                16 * 1024
                            },
                        ),
                        &mut omitted_properties,
                    )?,
                    0x0003 if is_distribution_list_checksum => omit_malformed(
                        scalar_property(job, mail, property, transient_id, property_type),
                        &mut omitted_properties,
                    )?
                    .flatten(),
                    _ => scalar_property(job, mail, property, transient_id, property_type)?,
                }
            };
            if is_public_keywords
                && matches!(
                    &value,
                    Some(RawPropertyValue::MultipleUnicode(values))
                        if !public_keywords_are_valid(values)
                )
            {
                value = None;
            }
            if let Some(value) = value {
                named_properties.push(NamedProperty { set, name, value });
            } else {
                partial = true;
                if omitted_properties == omissions_before_value {
                    omitted_properties = omitted_properties.saturating_add(1);
                }
                unsupported_properties.push(UnsupportedProperty {
                    id: transient_id,
                    property_type,
                    byte_len: property.blob.byte_len,
                });
            }
            continue;
        }
        let Some(id) = property
            .property_id
            .and_then(|value| u16::try_from(value).ok())
        else {
            partial = true;
            omitted_properties = omitted_properties.saturating_add(1);
            continue;
        };
        if property.record_set_index != 0 || id == 0 || id >= 0x8000 {
            partial = true;
            omitted_properties = omitted_properties.saturating_add(1);
            unsupported_properties.push(UnsupportedProperty {
                id,
                property_type,
                byte_len: property.blob.byte_len,
            });
            continue;
        }
        if matches!(id, 0x0E07 | 0x0E1F | 0x1016 | 0x3FDE) && !serialized_ids.insert(id) {
            partial = true;
            omitted_properties = omitted_properties.saturating_add(1);
            continue;
        }
        if id == 0x0E1F {
            if let Some(value) =
                omit_malformed(read_boolean(job, mail, property), &mut omitted_properties)?
            {
                rtf_in_sync = value;
                rtf_in_sync_observed = true;
            } else {
                partial = true;
            }
            continue;
        }
        if id == 0x1016 {
            if let Some(value) =
                omit_malformed(read_i32(job, mail, property), &mut omitted_properties)?
            {
                native_body = match value {
                    1 => Some(NativeBody::PlainText),
                    2 => Some(NativeBody::Rtf),
                    3 => Some(NativeBody::Html),
                    _ => {
                        partial = true;
                        omitted_properties = omitted_properties.saturating_add(1);
                        None
                    }
                };
            } else {
                partial = true;
            }
            continue;
        }
        if id == 0x0E07 {
            if let Some(value) =
                omit_malformed(read_i32(job, mail, property), &mut omitted_properties)?
            {
                message_flags = Some(value);
            } else {
                partial = true;
            }
            continue;
        }
        if id == 0x3FDE {
            if let Some(value) =
                omit_malformed(read_i32(job, mail, property), &mut omitted_properties)?
            {
                if value > 0 {
                    internet_codepage = Some(value);
                } else {
                    partial = true;
                    omitted_properties = omitted_properties.saturating_add(1);
                }
            } else {
                partial = true;
            }
            continue;
        }
        if matches!(id, 0x3007 | 0x3008) {
            match omit_malformed(
                scalar_property(job, mail, property, id, property_type),
                &mut omitted_properties,
            )? {
                Some(Some(RawPropertyValue::Time(value))) if id == 0x3007 => {
                    creation_filetime = Some(value);
                }
                Some(Some(RawPropertyValue::Time(value))) => {
                    modification_filetime = Some(value);
                }
                _ => partial = true,
            }
            continue;
        }
        if is_writer_managed(id) && !matches!(id, 0x007D | 0x1000 | 0x1009 | 0x1013) {
            continue;
        }
        if copied_contents_property_type(id).is_some_and(|expected| property_type != expected) {
            partial = true;
            omitted_properties = omitted_properties.saturating_add(1);
            unsupported_properties.push(UnsupportedProperty {
                id,
                property_type,
                byte_len: property.blob.byte_len,
            });
            continue;
        }
        if !serialized_ids.insert(id) {
            partial = true;
            omitted_properties = omitted_properties.saturating_add(1);
            continue;
        }
        if copied_contents_property_type(id).is_some() {
            match omit_malformed(
                materialize_copied_contents_property(job, mail, property, id, property_type),
                &mut omitted_properties,
            )? {
                Some(Some(value)) => raw_properties.push(RawProperty { id, value }),
                result => {
                    partial = true;
                    if result.is_some() {
                        omitted_properties = omitted_properties.saturating_add(1);
                    }
                    unsupported_properties.push(UnsupportedProperty {
                        id,
                        property_type,
                        byte_len: property.blob.byte_len,
                    });
                }
            }
            continue;
        }
        if property.blob.byte_len == 0 {
            match property_type {
                0x001F => raw_properties.push(RawProperty {
                    id,
                    value: RawPropertyValue::Unicode(String::new()),
                }),
                0x0102 => raw_properties.push(RawProperty {
                    id,
                    value: RawPropertyValue::Binary(Vec::new()),
                }),
                _ => {
                    partial = true;
                    omitted_properties = omitted_properties.saturating_add(1);
                    unsupported_properties.push(UnsupportedProperty {
                        id,
                        property_type,
                        byte_len: 0,
                    });
                }
            }
            continue;
        }
        match omit_malformed(
            scalar_property(job, mail, property, id, property_type),
            &mut omitted_properties,
        )? {
            Some(Some(value)) => raw_properties.push(RawProperty { id, value }),
            Some(None) => {
                if writer_stream_type_is_supported(property_type)
                    && property.blob.byte_len <= i32::MAX as u64
                    && body_type_is_valid(id, property_type)
                {
                    if omit_malformed(
                        validate_streamed_property(job, mail, property, id, property_type),
                        &mut omitted_properties,
                    )?
                    .is_none()
                    {
                        partial = true;
                        unsupported_properties.push(UnsupportedProperty {
                            id,
                            property_type,
                            byte_len: property.blob.byte_len,
                        });
                        continue;
                    }
                    spooled_properties.push(SpooledPropertySpec {
                        id,
                        property_type,
                        blob: file_blob(job, &property.blob)?,
                    });
                    if id == 0x1013 {
                        html_property = Some(property);
                    }
                } else {
                    partial = true;
                    omitted_properties = omitted_properties.saturating_add(1);
                    unsupported_properties.push(UnsupportedProperty {
                        id,
                        property_type,
                        byte_len: property.blob.byte_len,
                    });
                }
            }
            None => partial = true,
        }
    }
    if mail
        .message_class
        .as_deref()
        .is_some_and(distribution_list_message_class)
    {
        contain_distribution_list_properties(
            &mut named_properties,
            &mut partial,
            &mut omitted_properties,
        );
    }

    let source_internet_codepage = internet_codepage;
    let internet_codepage = source_internet_codepage.unwrap_or(65001);
    let mut html_has_utf8_evidence = false;
    if internet_codepage == 65001 {
        if let Some(property) = html_property {
            html_has_utf8_evidence = valid_utf8_property(job, mail, property)?;
            if !html_has_utf8_evidence {
                spooled_properties.retain(|property| property.id != 0x1013);
                partial = true;
                omitted_properties = omitted_properties.saturating_add(1);
                unsupported_properties.push(UnsupportedProperty {
                    id: 0x1013,
                    property_type: u16::try_from(property.value_type.unwrap_or_default())
                        .unwrap_or_default(),
                    byte_len: property.blob.byte_len,
                });
            }
        }
    }
    record_internet_codepage_provenance(
        source_internet_codepage,
        html_has_utf8_evidence,
        &mut reconstructions,
    );

    let mut recipient_property_count = 0_u64;
    for recipient in &mail.recipients {
        for property in &recipient.properties {
            let reconstructed =
                match recipient_property_is_reconstructed(job, mail, recipient, property) {
                    Ok(value) => value,
                    Err(
                        error
                        @ (CanonicalWriteError::Job(_) | CanonicalWriteError::BlobRead { .. }),
                    ) => return Err(error),
                    Err(
                        CanonicalWriteError::InvalidCandidate { .. }
                        | CanonicalWriteError::InvalidProperty { .. },
                    ) => false,
                };
            if !reconstructed {
                tracing::debug!(
                    property_id = property.property_id,
                    value_type = property.value_type,
                    record_set_index = property.record_set_index,
                    entry_index = property.entry_index,
                    byte_len = property.blob.byte_len,
                    "recipient property is not reconstructed exactly"
                );
                recipient_property_count = recipient_property_count.saturating_add(1);
            }
        }
    }
    let non_smtp_recipient = mail.recipients.iter().any(|recipient| {
        recipient
            .address_type
            .as_deref()
            .is_some_and(|value| !value.eq_ignore_ascii_case("SMTP"))
    });
    let invalid_recipient_metadata = mail.recipients.iter().any(|recipient| {
        !matches!(recipient.recipient_type, Some(1..=3))
            || recipient
                .display_name
                .as_deref()
                .or(recipient.email_address.as_deref())
                .is_none()
    });
    if non_smtp_recipient || invalid_recipient_metadata {
        partial = true;
    }
    if recipient_property_count != 0 {
        partial = true;
        omitted_properties = omitted_properties.saturating_add(recipient_property_count);
    }

    for recipient in &mail.recipients {
        match (
            recipient
                .display_name
                .as_deref()
                .filter(|value| !value.is_empty()),
            recipient
                .email_address
                .as_deref()
                .filter(|value| !value.is_empty()),
        ) {
            (None, Some(_)) => {
                reconstructions.record_derived(ReconstructedField::RecipientDisplayName);
            }
            (Some(_), None) => {
                reconstructions.record_derived(ReconstructedField::RecipientAddress);
            }
            _ => {}
        }
    }
    let recipients = mail
        .recipients
        .iter()
        .filter_map(|recipient| {
            let kind = match recipient.recipient_type {
                Some(1) => RecipientKind::To,
                Some(2) => RecipientKind::Cc,
                Some(3) => RecipientKind::Bcc,
                _ => return None,
            };
            let display_name = recipient
                .display_name
                .clone()
                .or_else(|| recipient.email_address.clone())?;
            let email_address = recipient
                .email_address
                .clone()
                .unwrap_or_else(|| display_name.clone());
            Some(RecipientSpec {
                kind,
                display_name,
                email_address,
            })
        })
        .collect::<Vec<_>>();
    if recipients.len() != mail.recipients.len() {
        partial = true;
    }

    contain_body_metadata(
        &mut native_body,
        &mut rtf_in_sync,
        rtf_in_sync_observed,
        &spooled_properties,
        &mut partial,
        &mut omitted_properties,
    );

    let mut attachments = Vec::new();
    for attachment in &mail.attachments {
        match translate_attachment(job, mail, attachment)? {
            Some(translated) => {
                partial |= translated.partial;
                omitted_properties =
                    omitted_properties.saturating_add(translated.omitted_properties);
                omitted_attachments =
                    omitted_attachments.saturating_add(translated.omitted_attachments);
                reconstructions.merge(translated.reconstructions);
                item_keys.extend(translated.item_keys);
                unsupported_item_keys.extend(translated.unsupported_item_keys);
                if let Some(attachment) = translated.attachment {
                    attachments.push(attachment);
                }
            }
            None => {
                partial = true;
                omitted_attachments = omitted_attachments.saturating_add(1);
            }
        }
    }

    let message_class = mail.message_class.clone().unwrap_or_else(|| {
        reconstructions.record_generated(ReconstructedField::MessageClass);
        "IPM.Note".to_owned()
    });
    if document_message_class(&message_class) && attachments.is_empty() {
        partial = true;
        reconstructions.record_generated(ReconstructedField::DocumentAttachment);
    }
    if !supported_message_class(&message_class)
        || (calendar_exception_message_class(&message_class) && !allow_calendar_exception)
    {
        return invalid(mail, format!("unsupported message class {message_class:?}"));
    }
    let subject = mail
        .subject
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            reconstructions.record_generated(ReconstructedField::Subject);
            String::new()
        });
    let sender_optional = sender_optional_message_class(&message_class);
    let source_sender_name = mail.sender_name.clone().filter(|value| !value.is_empty());
    let source_sender_email = mail.sender_email.clone().filter(|value| !value.is_empty());
    let (sender_name, sender_email) = match (source_sender_name, source_sender_email) {
        (Some(name), Some(email)) => (name, email),
        (Some(name), None) => {
            reconstructions.record_derived(ReconstructedField::SenderAddress);
            (name.clone(), name)
        }
        (None, Some(email)) => {
            reconstructions.record_derived(ReconstructedField::SenderName);
            (email.clone(), email)
        }
        (None, None) => {
            if !sender_optional {
                reconstructions.record_generated(ReconstructedField::SenderName);
                reconstructions.record_generated(ReconstructedField::SenderAddress);
            }
            (String::new(), String::new())
        }
    };
    let (sent_filetime, invalid_sent_filetime) = contained_filetime(mail.submit_filetime);
    let (received_filetime, invalid_received_filetime) = contained_filetime(mail.delivery_filetime);
    if mail.submit_filetime.is_none() || invalid_sent_filetime {
        reconstructions.record_generated(ReconstructedField::SubmitTime);
    }
    if mail.delivery_filetime.is_none() || invalid_received_filetime {
        reconstructions.record_generated(ReconstructedField::DeliveryTime);
    }
    let creation_filetime = creation_filetime.unwrap_or_else(|| {
        if mail.delivery_filetime.is_some() && !invalid_received_filetime {
            reconstructions.record_derived(ReconstructedField::CreationTime);
        } else {
            reconstructions.record_generated(ReconstructedField::CreationTime);
        }
        received_filetime
    });
    let modification_filetime = modification_filetime.unwrap_or_else(|| {
        if mail.delivery_filetime.is_some() && !invalid_received_filetime {
            reconstructions.record_derived(ReconstructedField::ModificationTime);
        } else {
            reconstructions.record_generated(ReconstructedField::ModificationTime);
        }
        received_filetime
    });
    let message_flags = message_flags.unwrap_or_else(|| {
        reconstructions.record_generated(ReconstructedField::MessageFlags);
        1
    });
    partial |= invalid_sent_filetime || invalid_received_filetime;
    omitted_properties = omitted_properties
        .saturating_add(u64::from(invalid_sent_filetime))
        .saturating_add(u64::from(invalid_received_filetime));
    Ok(TranslatedMessage {
        message: MessageSpec {
            message_class,
            message_flags,
            internet_codepage,
            subject,
            sender_name,
            sender_email,
            recipients,
            sent_filetime,
            received_filetime,
            creation_filetime,
            modification_filetime,
            body_text: None,
            body_html: None,
            body_rtf: None,
            native_body,
            rtf_in_sync,
            internet_headers: None,
            attachments,
            named_properties,
            raw_properties,
            spooled_properties,
            unsupported_properties,
        },
        item_keys,
        unsupported_item_keys,
        partial,
        omitted_properties,
        omitted_attachments,
        reconstructions,
    })
}

fn copied_contents_property_type(id: u16) -> Option<u16> {
    match id {
        0x0017 | 0x0036 | 0x0E38 | 0x1097 | 0x65C6 => Some(0x0003),
        0x0057 | 0x0058 => Some(0x000B),
        0x0070 => Some(0x001F),
        0x0071 | 0x0E3C | 0x0E3D | 0x3013 => Some(0x0102),
        _ => None,
    }
}

fn materialize_copied_contents_property(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    property: &CanonicalProperty,
    id: u16,
    property_type: u16,
) -> Result<Option<RawPropertyValue>, CanonicalWriteError> {
    const MAX_COPIED_PROPERTY_BYTES: u64 = 16 * 1024;
    if property.blob.byte_len > MAX_COPIED_PROPERTY_BYTES {
        return Ok(None);
    }
    match property_type {
        0x001F => {
            read_unicode(job, mail, property).map(|value| value.map(RawPropertyValue::Unicode))
        }
        0x0102 => read_blob(job, mail, property, MAX_COPIED_PROPERTY_BYTES)
            .map(|value| (!value.is_empty()).then_some(RawPropertyValue::Binary(value))),
        _ => scalar_property(job, mail, property, id, property_type),
    }
}

struct TranslatedAttachment {
    attachment: Option<AttachmentSpec>,
    item_keys: Vec<String>,
    unsupported_item_keys: Vec<String>,
    partial: bool,
    omitted_properties: u64,
    omitted_attachments: u64,
    reconstructions: ReconstructionCounts,
}

fn translate_attachment(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    attachment: &CanonicalAttachment,
) -> Result<Option<TranslatedAttachment>, CanonicalWriteError> {
    let mut mime_type = None;
    let mut content_id = None;
    let mut content_location = None;
    let mut rendering_position = None;
    let mut flags = 0;
    let mut flags_observed = false;
    let mut raw_properties = Vec::new();
    let mut omitted_properties = 0_u64;
    let mut mapped_property_ids = BTreeSet::new();
    let mut reconstructions = ReconstructionCounts::default();
    let parent_class = mail.message_class.as_deref().unwrap_or("IPM.Note");
    let preserves_calendar_exception = appointment_message_class(parent_class)
        && attachment
            .embedded
            .as_ref()
            .and_then(|message| message.message_class.as_deref())
            .is_some_and(calendar_exception_message_class)
        && calendar_exception_attachment_has_linkage(attachment);
    for property in &attachment.properties {
        let Some(property_id) = property.property_id else {
            omitted_properties = omitted_properties.saturating_add(1);
            continue;
        };
        if property.record_set_index != 0 || !mapped_property_ids.insert(property_id) {
            omitted_properties = omitted_properties.saturating_add(1);
            continue;
        }
        match property_id {
            0x370E => {
                mime_type =
                    omit_malformed(read_unicode(job, mail, property), &mut omitted_properties)?
                        .flatten();
            }
            0x3712 => {
                content_id =
                    omit_malformed(read_unicode(job, mail, property), &mut omitted_properties)?
                        .flatten();
            }
            0x3713 => {
                content_location =
                    omit_malformed(read_unicode(job, mail, property), &mut omitted_properties)?
                        .flatten();
            }
            0x370B => {
                rendering_position =
                    omit_malformed(read_i32(job, mail, property), &mut omitted_properties)?;
            }
            0x3714 => {
                if let Some(value) =
                    omit_malformed(read_i32(job, mail, property), &mut omitted_properties)?
                {
                    flags = value;
                    flags_observed = true;
                }
            }
            id if attachment_property_is_mapped(id) => {}
            id if preserves_calendar_exception
                && attachment_property_is_preservable(id)
                && property.named_property.is_none() =>
            {
                let Some(property_type) = omit_malformed(
                    checked_property_type(mail, property),
                    &mut omitted_properties,
                )?
                else {
                    continue;
                };
                if !attachment_property_type_is_preservable(id, property_type) {
                    omitted_properties = omitted_properties.saturating_add(1);
                    continue;
                }
                let id = u16::try_from(id).map_err(|_| CanonicalWriteError::InvalidProperty {
                    item_key: mail.durable_item_key.clone(),
                    property_id: id,
                    detail: "attachment property identifier is out of range".to_owned(),
                })?;
                let omissions_before_value = omitted_properties;
                let value = match property_type {
                    0x001F => match omit_malformed(
                        read_unicode(job, mail, property),
                        &mut omitted_properties,
                    )? {
                        Some(Some(value)) => Some(RawPropertyValue::Unicode(value)),
                        Some(None) => {
                            omitted_properties = omitted_properties.saturating_add(1);
                            None
                        }
                        None => None,
                    },
                    0x0102 if property.blob.byte_len <= 1_048_576 => Some(
                        RawPropertyValue::Binary(read_blob(job, mail, property, 1_048_576)?),
                    ),
                    _ => omit_malformed(
                        scalar_property(job, mail, property, id, property_type),
                        &mut omitted_properties,
                    )?
                    .flatten(),
                };
                if let Some(value) = value {
                    raw_properties.push(RawProperty { id, value });
                } else if omitted_properties == omissions_before_value {
                    omitted_properties = omitted_properties.saturating_add(1);
                }
            }
            _ => omitted_properties = omitted_properties.saturating_add(1),
        }
    }
    let (
        content,
        item_keys,
        unsupported_item_keys,
        child_partial,
        child_omitted_properties,
        child_omitted_attachments,
    ) = if let Some(embedded) = &attachment.embedded {
        let message_class = embedded.message_class.as_deref().unwrap_or("IPM.Note");
        let calendar_exception = calendar_exception_message_class(message_class);
        if !supported_message_class(message_class)
            || (calendar_exception && !preserves_calendar_exception)
            || (calendar_exception && !calendar_exception_raw_linkage_is_complete(&raw_properties))
        {
            let mut unsupported_item_keys = Vec::new();
            collect_message_item_keys(embedded, &mut unsupported_item_keys);
            return Ok(Some(TranslatedAttachment {
                attachment: None,
                item_keys: Vec::new(),
                unsupported_item_keys,
                partial: true,
                omitted_properties,
                omitted_attachments: 1,
                reconstructions,
            }));
        }
        let translated = translate_message(job, embedded, calendar_exception)?;
        reconstructions.merge(translated.reconstructions);
        (
            AttachmentContent::Embedded(Box::new(translated.message)),
            translated.item_keys,
            translated.unsupported_item_keys,
            translated.partial,
            translated.omitted_properties,
            translated.omitted_attachments,
        )
    } else if !attachment.data_complete {
        return Ok(None);
    } else if let Some(data) = &attachment.data {
        if data.byte_len > 2_147_483_647 {
            return Ok(None);
        }
        let content = if data.byte_len == 0 {
            AttachmentContent::Binary(Vec::new())
        } else {
            AttachmentContent::Spooled(file_blob(job, data)?)
        };
        (content, Vec::new(), Vec::new(), false, 0, 0)
    } else {
        return Ok(None);
    };
    let source_mime = mime_type.filter(|value| !value.is_empty());
    let source_filename = attachment
        .filename
        .clone()
        .filter(|value| !value.is_empty());
    let detected_mime =
        if attachment.embedded.is_none() && (source_mime.is_none() || source_filename.is_none()) {
            if let Some(data) = &attachment.data {
                infer_attachment_mime(job, mail, data, attachment.filename.as_deref())?
            } else {
                None
            }
        } else {
            None
        };
    let mime_type = if let Some(source) = source_mime {
        Some(source)
    } else if attachment.embedded.is_some() {
        reconstructions.record_derived(ReconstructedField::AttachmentMimeType);
        Some("message/rfc822".to_owned())
    } else {
        detected_mime.map(|value| {
            reconstructions.record_derived(ReconstructedField::AttachmentMimeType);
            value.to_owned()
        })
    };
    let filename = source_filename.unwrap_or_else(|| {
        reconstructions.record_generated(ReconstructedField::AttachmentFilename);
        if attachment.embedded.is_some() {
            format!("Embedded message {}.msg", attachment.index)
        } else {
            let extension = detected_mime
                .and_then(extension_for_mime)
                .or_else(|| mime_type.as_deref().and_then(extension_for_mime))
                .unwrap_or("bin");
            format!("Recovered attachment {}.{extension}", attachment.index)
        }
    });
    if rendering_position.is_none() {
        reconstructions.record_generated(ReconstructedField::AttachmentRenderingPosition);
    }
    if !flags_observed {
        reconstructions.record_generated(ReconstructedField::AttachmentFlags);
    }
    Ok(Some(TranslatedAttachment {
        attachment: Some(AttachmentSpec {
            filename,
            mime_type,
            content_id,
            content_location,
            rendering_position,
            flags,
            raw_properties,
            content,
        }),
        item_keys,
        unsupported_item_keys,
        partial: (attachment.embedded.is_none() && !attachment.data_complete)
            || omitted_properties != 0
            || child_partial
            || child_omitted_properties != 0
            || child_omitted_attachments != 0,
        omitted_properties: omitted_properties.saturating_add(child_omitted_properties),
        omitted_attachments: child_omitted_attachments,
        reconstructions,
    }))
}

fn collect_message_item_keys(message: &CanonicalMail, item_keys: &mut Vec<String>) {
    item_keys.push(message.durable_item_key.clone());
    for attachment in &message.attachments {
        if let Some(embedded) = &attachment.embedded {
            collect_message_item_keys(embedded, item_keys);
        }
    }
}

fn scalar_property(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    property: &CanonicalProperty,
    id: u16,
    property_type: u16,
) -> Result<Option<RawPropertyValue>, CanonicalWriteError> {
    let bytes = match property_type {
        0x0002 | 0x0003 | 0x0004 | 0x0005 | 0x0006 | 0x0007 | 0x000A | 0x000B | 0x0014 | 0x0040
        | 0x0048 => read_blob(job, mail, property, 16)?,
        _ => return Ok(None),
    };
    let malformed = || CanonicalWriteError::InvalidProperty {
        item_key: mail.durable_item_key.clone(),
        property_id: u32::from(id),
        detail: format!("type 0x{property_type:04X} has {} bytes", bytes.len()),
    };
    let value = match (property_type, bytes.as_slice()) {
        (0x0002, [a, b]) => RawPropertyValue::Integer16(i16::from_le_bytes([*a, *b])),
        (0x0003, [a, b, c, d]) => RawPropertyValue::Integer32(i32::from_le_bytes([*a, *b, *c, *d])),
        (0x0004, [a, b, c, d]) => {
            RawPropertyValue::Floating32(u32::from_le_bytes([*a, *b, *c, *d]))
        }
        (0x0005, bytes) if bytes.len() == 8 => RawPropertyValue::Floating64(u64_le(bytes)),
        (0x0006, bytes) if bytes.len() == 8 => RawPropertyValue::Currency(i64_le(bytes)),
        (0x0007, bytes) if bytes.len() == 8 => RawPropertyValue::FloatingTime(u64_le(bytes)),
        (0x000A, [a, b, c, d]) => RawPropertyValue::ErrorCode(u32::from_le_bytes([*a, *b, *c, *d])),
        // libpff exposes PT_BOOLEAN as one byte for PST values written in the
        // compact table representation, while property streams can contain
        // the two-byte MAPI representation.
        (0x000B, [value]) => RawPropertyValue::Boolean(*value != 0),
        (0x000B, [a, b]) => RawPropertyValue::Boolean(u16::from_le_bytes([*a, *b]) != 0),
        (0x0014, bytes) if bytes.len() == 8 => RawPropertyValue::Integer64(i64_le(bytes)),
        (0x0040, bytes) if bytes.len() == 8 => RawPropertyValue::Time(i64_le(bytes)),
        (0x0048, bytes) if bytes.len() == 16 => {
            let mut value = [0_u8; 16];
            value.copy_from_slice(bytes);
            RawPropertyValue::Guid(value)
        }
        _ => return Err(malformed()),
    };
    Ok(Some(value))
}

fn multiple_binary_property(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    property: &CanonicalProperty,
    id: u16,
    maximum_bytes: u64,
) -> Result<RawPropertyValue, CanonicalWriteError> {
    if property.blob.byte_len > maximum_bytes {
        return Err(CanonicalWriteError::InvalidProperty {
            item_key: mail.durable_item_key.clone(),
            property_id: u32::from(id),
            detail: format!(
                "PtypMultipleBinary has {} bytes, limit is {maximum_bytes}",
                property.blob.byte_len
            ),
        });
    }
    let bytes = read_blob(job, mail, property, maximum_bytes)?;
    decode_multiple_binary(&bytes)
        .map(RawPropertyValue::MultipleBinary)
        .map_err(|detail| CanonicalWriteError::InvalidProperty {
            item_key: mail.durable_item_key.clone(),
            property_id: u32::from(id),
            detail: detail.to_owned(),
        })
}

fn decode_multiple_binary(bytes: &[u8]) -> Result<Vec<Vec<u8>>, &'static str> {
    decode_variable_width_values(bytes)
        .map(|values| values.into_iter().map(ToOwned::to_owned).collect())
}

fn multiple_unicode_property(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    property: &CanonicalProperty,
    id: u16,
    maximum_bytes: u64,
) -> Result<RawPropertyValue, CanonicalWriteError> {
    if property.blob.byte_len > maximum_bytes {
        return Err(CanonicalWriteError::InvalidProperty {
            item_key: mail.durable_item_key.clone(),
            property_id: u32::from(id),
            detail: format!(
                "PtypMultipleString has {} bytes, limit is {maximum_bytes}",
                property.blob.byte_len
            ),
        });
    }
    let bytes = read_blob(job, mail, property, maximum_bytes)?;
    decode_multiple_unicode(&bytes)
        .map(RawPropertyValue::MultipleUnicode)
        .map_err(|detail| CanonicalWriteError::InvalidProperty {
            item_key: mail.durable_item_key.clone(),
            property_id: u32::from(id),
            detail: detail.to_owned(),
        })
}

fn decode_multiple_unicode(bytes: &[u8]) -> Result<Vec<String>, &'static str> {
    decode_variable_width_values(bytes)?
        .into_iter()
        .map(|value| {
            if value.len() % 2 != 0 {
                return Err("PtypMultipleString value has odd byte length");
            }
            let mut units = value
                .chunks_exact(2)
                .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
                .collect::<Vec<_>>();
            if let Some(end) = units.iter().position(|unit| *unit == 0) {
                if units[end + 1..].iter().any(|unit| *unit != 0) {
                    return Err("PtypMultipleString has data after its terminator");
                }
                units.truncate(end);
            }
            String::from_utf16(&units).map_err(|_| "PtypMultipleString value is invalid UTF-16")
        })
        .collect()
}

fn public_keywords_are_valid(values: &[String]) -> bool {
    values
        .iter()
        .all(|value| value.encode_utf16().count() < 256)
}

fn decode_variable_width_values(bytes: &[u8]) -> Result<Vec<&[u8]>, &'static str> {
    let count_bytes = bytes
        .get(..4)
        .ok_or("PtypMultipleBinary count is truncated")?;
    let count = usize::try_from(u32::from_le_bytes([
        count_bytes[0],
        count_bytes[1],
        count_bytes[2],
        count_bytes[3],
    ]))
    .map_err(|_| "PtypMultipleBinary count is out of range")?;
    let header_len = count
        .checked_add(1)
        .and_then(|value| value.checked_mul(4))
        .ok_or("PtypMultipleBinary header length overflows")?;
    if header_len > bytes.len() {
        return Err("PtypMultipleBinary offset table is truncated");
    }
    if count == 0 {
        if bytes.len() != 4 {
            return Err("empty PtypMultipleBinary has trailing bytes");
        }
        return Ok(Vec::new());
    }

    let mut offsets = Vec::with_capacity(count);
    for index in 0..count {
        let start = 4_usize
            .checked_add(
                index
                    .checked_mul(4)
                    .ok_or("PtypMultipleBinary offset index overflows")?,
            )
            .ok_or("PtypMultipleBinary offset index overflows")?;
        let encoded = bytes
            .get(start..start + 4)
            .ok_or("PtypMultipleBinary offset is truncated")?;
        offsets.push(
            usize::try_from(u32::from_le_bytes([
                encoded[0], encoded[1], encoded[2], encoded[3],
            ]))
            .map_err(|_| "PtypMultipleBinary offset is out of range")?,
        );
    }
    if offsets.first().copied() != Some(header_len)
        || offsets
            .windows(2)
            .any(|pair| pair[0] > pair[1] || pair[1] > bytes.len())
        || offsets.last().is_some_and(|offset| *offset > bytes.len())
    {
        return Err("PtypMultipleBinary offsets are not ordered within the property");
    }
    offsets
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = offsets.get(index + 1).copied().unwrap_or(bytes.len());
            bytes
                .get(*start..end)
                .ok_or("PtypMultipleBinary value range is invalid")
        })
        .collect()
}

fn contain_distribution_list_properties(
    properties: &mut Vec<NamedProperty>,
    partial: &mut bool,
    omitted_properties: &mut u64,
) {
    let key_matches = |property: &NamedProperty, lid| {
        property.set == NamedPropertySet::Guid(PSETID_ADDRESS)
            && property.name == NamedPropertyName::Numeric(lid)
    };
    let members = properties
        .iter()
        .find(|property| key_matches(property, 0x8055))
        .map(|property| match &property.value {
            RawPropertyValue::MultipleBinary(values) => Ok(values.len()),
            _ => Err(()),
        });
    let one_off_members = properties
        .iter()
        .find(|property| key_matches(property, 0x8054))
        .map(|property| match &property.value {
            RawPropertyValue::MultipleBinary(values) => Ok(values.len()),
            _ => Err(()),
        });
    let primary_is_invalid = matches!(members, Some(Err(())));
    let member_count = members.and_then(Result::ok);
    let remove_one_off = one_off_members.is_some()
        && (primary_is_invalid || one_off_members.and_then(Result::ok) != member_count);
    let remove_checksum = member_count.is_none()
        || properties
            .iter()
            .find(|property| key_matches(property, 0x804C))
            .is_some_and(|property| !matches!(property.value, RawPropertyValue::Integer32(_)));

    properties.retain(|property| {
        let remove = (primary_is_invalid && key_matches(property, 0x8055))
            || ((primary_is_invalid || remove_one_off) && key_matches(property, 0x8054))
            || (remove_checksum && key_matches(property, 0x804C));
        if remove {
            *omitted_properties = omitted_properties.saturating_add(1);
            *partial = true;
        }
        !remove
    });
}

fn read_boolean(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    property: &CanonicalProperty,
) -> Result<bool, CanonicalWriteError> {
    let property_type = checked_property_type(mail, property)?;
    match scalar_property(job, mail, property, 0x0E1F, property_type)? {
        Some(RawPropertyValue::Boolean(value)) => Ok(value),
        _ => invalid(mail, "RTF sync property is not Boolean"),
    }
}

fn read_i32(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    property: &CanonicalProperty,
) -> Result<i32, CanonicalWriteError> {
    let property_type = checked_property_type(mail, property)?;
    let id = property
        .property_id
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(0);
    match scalar_property(job, mail, property, id, property_type)? {
        Some(RawPropertyValue::Integer32(value)) => Ok(value),
        _ => invalid(mail, "property is not a 32-bit integer"),
    }
}

fn read_unicode(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    property: &CanonicalProperty,
) -> Result<Option<String>, CanonicalWriteError> {
    if property.value_type != Some(0x001F) || property.blob.byte_len > 1_048_576 {
        return invalid(mail, "attachment text property is not bounded Unicode");
    }
    let bytes = read_blob(job, mail, property, 1_048_576)?;
    if bytes.len() % 2 != 0 {
        return invalid(mail, "attachment Unicode property has odd byte length");
    }
    let mut words = bytes
        .chunks_exact(2)
        .map(|word| u16::from_le_bytes([word[0], word[1]]))
        .collect::<Vec<_>>();
    if words.last() == Some(&0) {
        words.pop();
    }
    let value = String::from_utf16(&words).map_err(|_| CanonicalWriteError::InvalidProperty {
        item_key: mail.durable_item_key.clone(),
        property_id: property.property_id.unwrap_or(0),
        detail: "invalid UTF-16".to_owned(),
    })?;
    Ok((!value.is_empty()).then_some(value))
}

fn read_blob(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    property: &CanonicalProperty,
    maximum: u64,
) -> Result<Vec<u8>, CanonicalWriteError> {
    if property.blob.byte_len > maximum {
        return invalid(
            mail,
            "fixed-width property exceeds its materialization bound",
        );
    }
    let file = job.open_blob(&property.blob)?;
    let capacity = usize::try_from(property.blob.byte_len).map_err(|_| {
        CanonicalWriteError::InvalidCandidate {
            item_key: mail.durable_item_key.clone(),
            detail: "property length does not fit memory index".to_owned(),
        }
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(property.blob.byte_len)
        .read_to_end(&mut bytes)
        .map_err(|source| CanonicalWriteError::BlobRead {
            item_key: mail.durable_item_key.clone(),
            source,
        })?;
    if bytes.len() != capacity {
        return invalid(mail, "property length changed while reading");
    }
    Ok(bytes)
}

fn file_blob(
    job: &TranslationSource<'_>,
    blob: &SpooledBlob,
) -> Result<FileBlobSpec, CanonicalWriteError> {
    Ok(FileBlobSpec {
        path: job.verified_blob_path(blob)?,
        offset: blob.pack_offset.unwrap_or(0),
        byte_len: blob.byte_len,
        sha256: decode_sha256(&blob.sha256)?,
    })
}

fn decode_sha256(value: &str) -> Result<[u8; 32], CanonicalWriteError> {
    if value.len() != 64 {
        return Err(CanonicalWriteError::InvalidCandidate {
            item_key: "<job>".to_owned(),
            detail: "invalid SHA-256 digest".to_owned(),
        });
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (hex(pair[0])? << 4) | hex(pair[1])?;
    }
    Ok(output)
}

fn named_property_set(guid: [u8; 16]) -> NamedPropertySet {
    // libpff exposes the reserved NAMEID_GUID_MAPI selector as a zero GUID.
    // A literal custom zero GUID is not a valid named-property set identity.
    const LIBPFF_RESERVED_MAPI: [u8; 16] = [0; 16];
    const PS_MAPI: [u8; 16] = [
        0x28, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    const PS_PUBLIC_STRINGS: [u8; 16] = [
        0x29, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    match guid {
        LIBPFF_RESERVED_MAPI | PS_MAPI => NamedPropertySet::Mapi,
        PS_PUBLIC_STRINGS => NamedPropertySet::PublicStrings,
        value => NamedPropertySet::Guid(value),
    }
}

fn hex(value: u8) -> Result<u8, CanonicalWriteError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(CanonicalWriteError::InvalidCandidate {
            item_key: "<job>".to_owned(),
            detail: "invalid SHA-256 digest".to_owned(),
        }),
    }
}

fn part_record_key(
    source_sha256: &str,
    recovery_mode: &str,
    maximum_pst_bytes: u64,
    part_index: u32,
) -> Result<[u8; 16], CanonicalWriteError> {
    if !matches!(recovery_mode, "balanced" | "aggressive") {
        return Err(CanonicalWriteError::InvalidCandidate {
            item_key: "<job>".to_owned(),
            detail: "invalid recovery mode for store identity".to_owned(),
        });
    }
    let source = decode_sha256(source_sha256)?;
    let digest = Sha256::new()
        .chain_update(b"pstforge/store-record-key/v1\0")
        .chain_update(source)
        .chain_update(recovery_mode.as_bytes())
        .chain_update(b"\0unicode-pst-v1\0")
        .chain_update(maximum_pst_bytes.to_le_bytes())
        .chain_update(part_index.to_le_bytes())
        .finalize();
    let mut record_key = [0_u8; 16];
    record_key.copy_from_slice(&digest[..16]);
    Ok(record_key)
}

fn contained_filetime(value: Option<u64>) -> (i64, bool) {
    match value {
        Some(value) => match i64::try_from(value) {
            Ok(value) => (value, false),
            Err(_) => (0, true),
        },
        None => (0, false),
    }
}

fn record_internet_codepage_provenance(
    source: Option<i32>,
    html_has_utf8_evidence: bool,
    reconstructions: &mut ReconstructionCounts,
) {
    if source.is_some() {
        return;
    }
    if html_has_utf8_evidence {
        reconstructions.record_derived(ReconstructedField::InternetCodepage);
    } else {
        reconstructions.record_generated(ReconstructedField::InternetCodepage);
    }
}

fn infer_attachment_mime(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    blob: &SpooledBlob,
    filename: Option<&str>,
) -> Result<Option<&'static str>, CanonicalWriteError> {
    let file = job.open_blob(blob)?;
    let mut range =
        BlobRange::new(file, blob.byte_len).map_err(|source| CanonicalWriteError::BlobRead {
            item_key: mail.durable_item_key.clone(),
            source,
        })?;
    attachment_mime::detect(&mut range, blob.byte_len, filename).map_err(|source| {
        CanonicalWriteError::BlobRead {
            item_key: mail.durable_item_key.clone(),
            source,
        }
    })
}

fn extension_for_mime(value: &str) -> Option<&'static str> {
    let media_type = value.split(';').next()?.trim();
    if media_type.eq_ignore_ascii_case("application/pdf") {
        Some("pdf")
    } else if media_type.eq_ignore_ascii_case("image/png") {
        Some("png")
    } else if media_type.eq_ignore_ascii_case("image/jpeg") {
        Some("jpg")
    } else if media_type.eq_ignore_ascii_case("image/gif") {
        Some("gif")
    } else if media_type.eq_ignore_ascii_case("image/tiff") {
        Some("tif")
    } else if media_type.eq_ignore_ascii_case("application/zip") {
        Some("zip")
    } else if media_type.eq_ignore_ascii_case(
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ) {
        Some("docx")
    } else if media_type
        .eq_ignore_ascii_case("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
    {
        Some("xlsx")
    } else if media_type.eq_ignore_ascii_case(
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ) {
        Some("pptx")
    } else if media_type.eq_ignore_ascii_case("application/msword") {
        Some("doc")
    } else if media_type.eq_ignore_ascii_case("application/vnd.ms-excel") {
        Some("xls")
    } else if media_type.eq_ignore_ascii_case("application/vnd.ms-powerpoint") {
        Some("ppt")
    } else {
        None
    }
}

fn body_type_is_valid(id: u16, property_type: u16) -> bool {
    match id {
        0x007D | 0x1000 => matches!(property_type, 0x001E | 0x001F),
        0x1009 | 0x1013 => property_type == 0x0102,
        _ => true,
    }
}

fn writer_stream_type_is_supported(property_type: u16) -> bool {
    matches!(property_type, 0x001E | 0x001F | 0x0102)
}

fn supported_message_class(value: &str) -> bool {
    !value.is_empty()
}

fn contact_message_class(value: &str) -> bool {
    class_is_or_descends_from(value, "IPM.Contact")
}

fn distribution_list_message_class(value: &str) -> bool {
    class_is_or_descends_from(value, "IPM.DistList")
}

fn document_message_class(value: &str) -> bool {
    class_descends_from(value, "IPM.Document")
}

fn appointment_message_class(value: &str) -> bool {
    class_is_or_descends_from(value, "IPM.Appointment")
}

fn meeting_message_class(value: &str) -> bool {
    class_descends_from(value, "IPM.Schedule.Meeting")
}

fn task_message_class(value: &str) -> bool {
    class_is_or_descends_from(value, "IPM.Task")
}

fn sticky_note_message_class(value: &str) -> bool {
    class_is_or_descends_from(value, "IPM.StickyNote")
}

fn calendar_exception_message_class(value: &str) -> bool {
    value.eq_ignore_ascii_case("IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}")
}

fn sender_optional_message_class(value: &str) -> bool {
    !(class_is_or_descends_from(value, "IPM.Note")
        || class_descends_from(value, "REPORT.IPM.Note")
        || meeting_message_class(value))
        || contact_message_class(value)
        || appointment_message_class(value)
        || task_message_class(value)
        || sticky_note_message_class(value)
}

fn default_container_class(value: &str) -> &'static str {
    if contact_message_class(value) || distribution_list_message_class(value) {
        "IPF.Contact"
    } else if appointment_message_class(value) {
        "IPF.Appointment"
    } else if task_message_class(value) {
        "IPF.Task"
    } else if sticky_note_message_class(value) {
        "IPF.StickyNote"
    } else {
        "IPF.Note"
    }
}

fn class_is_or_descends_from(value: &str, root: &str) -> bool {
    value.eq_ignore_ascii_case(root) || class_descends_from(value, root)
}

fn class_descends_from(value: &str, root: &str) -> bool {
    value
        .get(..root.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(root))
        && value.as_bytes().get(root.len()) == Some(&b'.')
        && value.len() > root.len() + 1
}

fn validate_streamed_property(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    property: &CanonicalProperty,
    id: u16,
    property_type: u16,
) -> Result<(), CanonicalWriteError> {
    const MAX_RTF_CONTAINER_BYTES: u64 = 8 * 1024 * 1024;
    if id == 0x1009 {
        if property.blob.byte_len > MAX_RTF_CONTAINER_BYTES {
            return invalid(
                mail,
                "compressed RTF length is outside the validation bound",
            );
        }
        let bytes = read_blob(job, mail, property, MAX_RTF_CONTAINER_BYTES)?;
        return valid_compressed_rtf_container(&bytes)
            .then_some(())
            .ok_or_else(|| CanonicalWriteError::InvalidProperty {
                item_key: mail.durable_item_key.clone(),
                property_id: u32::from(id),
                detail: "invalid compressed RTF container".to_owned(),
            });
    }

    let file = job.open_blob(&property.blob)?;
    let mut reader = file.take(property.blob.byte_len);
    let valid = match property_type {
        0x001F => valid_utf16_stream(&mut reader)?,
        // String8 uses the message code page, and Binary has no internal PST
        // structure. Their bytes are safe to preserve without reinterpretation.
        0x001E | 0x0102 => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        invalid(mail, "streamed property payload is malformed")
    }
}

fn valid_compressed_rtf_container(bytes: &[u8]) -> bool {
    const MAX_RTF_RAW_BYTES: u64 = 64 * 1024 * 1024;
    const COMPRESSED: u32 = 0x7546_5A4C;
    const UNCOMPRESSED: u32 = 0x414C_454D;
    let Some(header) = bytes.get(..16) else {
        return false;
    };
    let compressed_size = u64::from(u32::from_le_bytes([
        header[0], header[1], header[2], header[3],
    ]));
    let raw_size = u64::from(u32::from_le_bytes([
        header[4], header[5], header[6], header[7],
    ]));
    let compression_type = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let crc = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    let byte_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if compressed_size != byte_len.saturating_sub(4)
        || raw_size > MAX_RTF_RAW_BYTES
        || !matches!(compression_type, COMPRESSED | UNCOMPRESSED)
        || (compression_type == UNCOMPRESSED
            && (raw_size != byte_len.saturating_sub(16) || crc != 0))
        || (compression_type == COMPRESSED && !compressed_rtf_has_end_run(&bytes[16..], raw_size))
    {
        return false;
    }
    compressed_rtf::decompress_rtf(bytes)
        .is_ok_and(|decoded| u64::try_from(decoded.encode_utf16().count()).ok() == Some(raw_size))
}

fn compressed_rtf_has_end_run(payload: &[u8], raw_size: u64) -> bool {
    const INITIAL_DICTIONARY_BYTES: u64 = 207;
    const DICTIONARY_SIZE: u64 = 4096;
    let mut cursor = 0_usize;
    let mut written = 0_u64;
    let mut write_offset = INITIAL_DICTIONARY_BYTES;

    while cursor < payload.len() {
        let control = payload[cursor];
        cursor += 1;
        for bit in 0..8 {
            if control & (1 << bit) == 0 {
                if cursor >= payload.len() || written >= raw_size {
                    return false;
                }
                cursor += 1;
                written += 1;
                write_offset = (write_offset + 1) % DICTIONARY_SIZE;
                continue;
            }

            let Some(reference) = payload.get(cursor..cursor.saturating_add(2)) else {
                return false;
            };
            cursor += 2;
            let encoded = u16::from_be_bytes([reference[0], reference[1]]);
            let offset = u64::from(encoded >> 4);
            if offset == write_offset {
                return written == raw_size;
            }
            let length = u64::from((encoded & 0x000F) + 2);
            let Some(next_written) = written.checked_add(length) else {
                return false;
            };
            if next_written > raw_size {
                return false;
            }
            written = next_written;
            write_offset = (write_offset + length) % DICTIONARY_SIZE;
        }
    }
    false
}

fn valid_utf16_stream(reader: &mut impl Read) -> Result<bool, CanonicalWriteError> {
    let mut pending_byte = None;
    let mut pending_high_surrogate = false;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| CanonicalWriteError::BlobRead {
                item_key: "<validation>".to_owned(),
                source,
            })?;
        if read == 0 {
            return Ok(pending_byte.is_none() && !pending_high_surrogate);
        }
        let mut bytes = buffer[..read].iter().copied();
        if let Some(low) = pending_byte.take() {
            let Some(high) = bytes.next() else {
                pending_byte = Some(low);
                continue;
            };
            let word = u16::from_le_bytes([low, high]);
            if !valid_utf16_word(word, &mut pending_high_surrogate) {
                return Ok(false);
            }
        }
        while let Some(low) = bytes.next() {
            let Some(high) = bytes.next() else {
                pending_byte = Some(low);
                break;
            };
            let word = u16::from_le_bytes([low, high]);
            if !valid_utf16_word(word, &mut pending_high_surrogate) {
                return Ok(false);
            }
        }
    }
}

fn valid_utf8_property(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    property: &CanonicalProperty,
) -> Result<bool, CanonicalWriteError> {
    let file = job.open_blob(&property.blob)?;
    valid_utf8_stream(
        &mut file.take(property.blob.byte_len),
        &mail.durable_item_key,
    )
}

fn valid_utf8_stream(reader: &mut impl Read, item_key: &str) -> Result<bool, CanonicalWriteError> {
    let mut pending = Vec::with_capacity(3);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| CanonicalWriteError::BlobRead {
                item_key: item_key.to_owned(),
                source,
            })?;
        if read == 0 {
            return Ok(pending.is_empty());
        }
        let mut bytes = Vec::with_capacity(pending.len() + read);
        bytes.extend_from_slice(&pending);
        bytes.extend_from_slice(&buffer[..read]);
        match std::str::from_utf8(&bytes) {
            Ok(_) => pending.clear(),
            Err(error) if error.error_len().is_some() => return Ok(false),
            Err(error) => {
                pending.clear();
                pending.extend_from_slice(&bytes[error.valid_up_to()..]);
                if pending.len() > 3 {
                    return Ok(false);
                }
            }
        }
    }
}

fn valid_utf16_word(word: u16, pending_high_surrogate: &mut bool) -> bool {
    if *pending_high_surrogate {
        if (0xDC00..=0xDFFF).contains(&word) {
            *pending_high_surrogate = false;
            true
        } else {
            false
        }
    } else if (0xD800..=0xDBFF).contains(&word) {
        *pending_high_surrogate = true;
        true
    } else {
        !(0xDC00..=0xDFFF).contains(&word)
    }
}

fn contain_body_metadata(
    native_body: &mut Option<NativeBody>,
    rtf_in_sync: &mut bool,
    rtf_in_sync_observed: bool,
    properties: &[SpooledPropertySpec],
    partial: &mut bool,
    omitted_properties: &mut u64,
) {
    let has_property = |id| properties.iter().any(|value| value.id == id);
    if rtf_in_sync_observed && !has_property(0x1009) {
        *rtf_in_sync = false;
        *partial = true;
        *omitted_properties = omitted_properties.saturating_add(1);
    }
    let native_body_is_present = match native_body {
        Some(NativeBody::PlainText) => has_property(0x1000),
        Some(NativeBody::Rtf) => has_property(0x1009),
        Some(NativeBody::Html) => has_property(0x1013),
        None => true,
    };
    if !native_body_is_present {
        *native_body = None;
        *partial = true;
        *omitted_properties = omitted_properties.saturating_add(1);
    }
}

fn is_writer_managed(id: u16) -> bool {
    matches!(
        id,
        0x001A
            | 0x0037
            | 0x0039
            | 0x0042
            | 0x0064
            | 0x0065
            | 0x007D
            | 0x0C1A
            | 0x0C1E
            | 0x0C1F
            | 0x0E02
            | 0x0E03
            | 0x0E04
            | 0x0E06
            | 0x0E07
            | 0x0E08
            | 0x0E17
            | 0x0E1B
            | 0x0E1F
            | 0x1000
            | 0x1009
            | 0x1013
            | 0x1016
            | 0x3007
            | 0x3008
            | 0x300B
            | 0x3FDE
    )
}

fn recipient_property_is_reconstructed(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    recipient: &CanonicalRecipient,
    property: &CanonicalProperty,
) -> Result<bool, CanonicalWriteError> {
    if property.record_set_index != recipient.index {
        return Ok(false);
    }
    let Some(id) = property
        .property_id
        .and_then(|value| u16::try_from(value).ok())
    else {
        return Ok(false);
    };
    if expected_recipient_property_value(recipient, id).is_none() {
        return Ok(false);
    }
    let Some(property_type) = property
        .value_type
        .and_then(|value| u16::try_from(value).ok())
    else {
        return Ok(false);
    };
    let actual = match property_type {
        0x001F => read_unicode(job, mail, property)?.map(RawPropertyValue::Unicode),
        0x0102 => Some(RawPropertyValue::Binary(read_blob(
            job,
            mail,
            property,
            16 * 1024,
        )?)),
        _ => scalar_property(job, mail, property, id, property_type)?,
    };
    Ok(actual
        .as_ref()
        .is_some_and(|actual| recipient_property_value_matches(recipient, id, actual)))
}

fn recipient_property_value_matches(
    recipient: &CanonicalRecipient,
    id: u16,
    actual: &RawPropertyValue,
) -> bool {
    expected_recipient_property_value(recipient, id).as_ref() == Some(actual)
}

fn expected_recipient_property_value(
    recipient: &CanonicalRecipient,
    id: u16,
) -> Option<RawPropertyValue> {
    let display_name = recipient
        .display_name
        .as_ref()
        .or(recipient.email_address.as_ref())?;
    let email_address = recipient.email_address.as_ref().unwrap_or(display_name);
    match id {
        0x0C15 => recipient
            .recipient_type
            .and_then(|value| i32::try_from(value).ok())
            .map(RawPropertyValue::Integer32),
        0x0E0F | 0x3A40 => Some(RawPropertyValue::Boolean(false)),
        0x0FF9 | 0x0FFF | 0x300B => Some(RawPropertyValue::Binary(Vec::new())),
        0x0FFE => Some(RawPropertyValue::Integer32(6)),
        0x3001 => Some(RawPropertyValue::Unicode(display_name.clone())),
        0x3002 => Some(RawPropertyValue::Unicode("SMTP".to_owned())),
        0x3003 | 0x39FF => Some(RawPropertyValue::Unicode(email_address.clone())),
        0x3900 => Some(RawPropertyValue::Integer32(0)),
        0x67F3 => Some(RawPropertyValue::Integer32(1)),
        0x67F2 => recipient
            .index
            .checked_add(1)
            .and_then(|value| i32::try_from(value).ok())
            .map(RawPropertyValue::Integer32),
        _ => None,
    }
}

fn attachment_property_is_mapped(id: u32) -> bool {
    matches!(id, 0x0E20 | 0x0E21 | 0x3701 | 0x3704 | 0x3705 | 0x3707)
}

fn attachment_property_is_preservable(id: u32) -> bool {
    matches!(id, 0x3001 | 0x3702 | 0x3709 | 0x7FFA..=0x7FFF)
}

fn attachment_property_type_is_preservable(id: u32, property_type: u16) -> bool {
    matches!(
        (id, property_type),
        (0x3001, 0x001F)
            | (0x3702 | 0x3709, 0x0102)
            | (0x7FFA | 0x7FFD, 0x0003)
            | (0x7FFB | 0x7FFC, 0x0040)
            | (0x7FFE | 0x7FFF, 0x000B)
    )
}

fn calendar_exception_attachment_has_linkage(attachment: &CanonicalAttachment) -> bool {
    (0x7FFA..=0x7FFE).all(|id| {
        attachment
            .properties
            .iter()
            .any(|property| property.record_set_index == 0 && property.property_id == Some(id))
    })
}

fn calendar_exception_raw_linkage_is_complete(properties: &[RawProperty]) -> bool {
    (0x7FFA..=0x7FFE).all(|id| {
        properties
            .iter()
            .any(|property| u32::from(property.id) == id)
    })
}

fn checked_property_type(
    mail: &CanonicalMail,
    property: &CanonicalProperty,
) -> Result<u16, CanonicalWriteError> {
    property
        .value_type
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| CanonicalWriteError::InvalidProperty {
            item_key: mail.durable_item_key.clone(),
            property_id: property.property_id.unwrap_or(0),
            detail: "property type is absent or out of range".to_owned(),
        })
}

fn omit_malformed<T>(
    result: Result<T, CanonicalWriteError>,
    omitted_properties: &mut u64,
) -> Result<Option<T>, CanonicalWriteError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error @ (CanonicalWriteError::Job(_) | CanonicalWriteError::BlobRead { .. })) => {
            Err(error)
        }
        Err(
            CanonicalWriteError::InvalidCandidate { .. }
            | CanonicalWriteError::InvalidProperty { .. },
        ) => {
            *omitted_properties = omitted_properties.saturating_add(1);
            Ok(None)
        }
    }
}

fn u64_le(bytes: &[u8]) -> u64 {
    let mut value = [0_u8; 8];
    value.copy_from_slice(bytes);
    u64::from_le_bytes(value)
}

fn i64_le(bytes: &[u8]) -> i64 {
    let mut value = [0_u8; 8];
    value.copy_from_slice(bytes);
    i64::from_le_bytes(value)
}

fn invalid<T>(mail: &CanonicalMail, detail: impl Into<String>) -> Result<T, CanonicalWriteError> {
    Err(CanonicalWriteError::InvalidCandidate {
        item_key: mail.durable_item_key.clone(),
        detail: detail.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use std::io::Cursor;

    use libpff_sys::{
        CatalogEvent, CatalogProvenance, CatalogSink, NamedPropertyIdentity,
        NamedPropertyName as LibpffNamedPropertyName, PropertyDescriptor, PropertyOwner,
        RecoveryUnit,
    };
    use pstforge_job::{DurableCatalogSink, ReconstructedField, ReconstructionCounts, SpooledBlob};
    use tempfile::tempdir;

    use super::{
        CanonicalWriteError, PartBuildOptions, attachment_property_type_is_preservable,
        build_part_writer_input, build_part_writer_input_with_folders_interruptible,
        contain_body_metadata, contain_distribution_list_properties, decode_multiple_binary,
        decode_multiple_unicode, default_container_class, document_message_class,
        extension_for_mime, named_property_set, normalize_associated_display_name,
        public_keywords_are_valid, recipient_property_value_matches,
        record_internet_codepage_provenance, supported_message_class,
        valid_compressed_rtf_container, valid_utf8_stream, valid_utf16_stream,
        writer_stream_type_is_supported,
    };
    use crate::{
        CanonicalAttachment, CanonicalFolder, CanonicalFolderLocation, CanonicalFolderRole,
        CanonicalMail, CanonicalMessagePlacement, CanonicalProperty, CanonicalRecipient,
        ContentCompleteness, ItemKey, RecoveryProvenance,
        attachment_mime::flat_signature as mime_from_signature,
    };
    use pstforge_pst::writer::{
        AttachmentContent, MailFolderRole, NamedProperty, NamedPropertyName, NamedPropertySet,
        NativeBody, RawPropertyValue, validate_mail_store_input,
    };

    #[test]
    fn libpff_reserved_mapi_guid_is_normalized() {
        assert_eq!(named_property_set([0; 16]), NamedPropertySet::Mapi);
    }

    #[test]
    fn multiple_binary_offsets_are_bounded_and_exact() {
        let bytes = [3, 0, 0, 0, 16, 0, 0, 0, 18, 0, 0, 0, 18, 0, 0, 0, 1, 2, 3];
        assert_eq!(
            decode_multiple_binary(&bytes),
            Ok(vec![vec![1, 2], Vec::new(), vec![3]])
        );
        assert_eq!(decode_multiple_binary(&[0, 0, 0, 0]), Ok(Vec::new()));
        assert!(decode_multiple_binary(&[]).is_err());
        assert!(decode_multiple_binary(&[0, 0, 0, 0, 1]).is_err());
        assert!(decode_multiple_binary(&[1, 0, 0, 0, 7, 0, 0, 0]).is_err());
        assert!(decode_multiple_binary(&[2, 0, 0, 0, 12, 0, 0, 0, 11, 0, 0, 0,]).is_err());
    }

    #[test]
    fn multiple_unicode_offsets_and_utf16_are_bounded_and_exact() {
        let bytes = [
            2, 0, 0, 0, 12, 0, 0, 0, 14, 0, 0, 0, b'A', 0, 0xAC, 0x20, 0x16, 0x4E, 0x4C, 0x75,
        ];
        assert_eq!(
            decode_multiple_unicode(&bytes),
            Ok(vec!["A".to_owned(), "\u{20AC}\u{4E16}\u{754C}".to_owned()])
        );
        assert_eq!(decode_multiple_unicode(&[0, 0, 0, 0]), Ok(Vec::new()));
        assert!(decode_multiple_unicode(&[1, 0, 0, 0, 8, 0, 0, 0, b'A']).is_err());
        assert!(decode_multiple_unicode(&[1, 0, 0, 0, 8, 0, 0, 0, 0x3D, 0xD8]).is_err());
        assert!(decode_multiple_unicode(&[1, 0, 0, 0, 7, 0, 0, 0]).is_err());
        assert_eq!(
            decode_multiple_unicode(&[1, 0, 0, 0, 8, 0, 0, 0, b'A', 0, 0, 0]),
            Ok(vec!["A".to_owned()])
        );
        assert!(
            decode_multiple_unicode(&[1, 0, 0, 0, 8, 0, 0, 0, b'A', 0, 0, 0, b'B', 0,]).is_err()
        );
    }

    #[test]
    fn public_keywords_limit_counts_utf16_units() {
        let valid = "\u{1F600}".repeat(127);
        let invalid = "\u{1F600}".repeat(128);
        assert!(public_keywords_are_valid(&[valid]));
        assert!(!public_keywords_are_valid(&[invalid]));
    }

    #[test]
    fn document_class_requires_a_dotted_suffix_and_reports_missing_attachment()
    -> Result<(), Box<dyn std::error::Error>> {
        fn mail(class: &str, key: &str) -> CanonicalMail {
            CanonicalMail {
                durable_item_key: key.to_owned(),
                key: ItemKey {
                    provenance: RecoveryProvenance::Recovered,
                    source_node_id: None,
                    recovery_index: Some(0),
                    occurrence: 0,
                },
                folder_path: vec!["Documents".to_owned()],
                folder_location: CanonicalFolderLocation::IpmSubtree,
                folder_role: CanonicalFolderRole::Ordinary,
                placement: CanonicalMessagePlacement::Normal,
                message_class: Some(class.to_owned()),
                subject: Some("Document containment checkpoint".to_owned()),
                sender_name: None,
                sender_email: None,
                submit_filetime: None,
                delivery_filetime: None,
                recipients: Vec::new(),
                attachments: Vec::new(),
                properties: Vec::new(),
                completeness: ContentCompleteness::Complete,
                spooled_bytes: 0,
            }
        }

        fn attachment(index: u32, data_complete: bool) -> CanonicalAttachment {
            CanonicalAttachment {
                index,
                attachment_type: Some(i32::from(b'd')),
                filename: Some(format!("document-{index}.bin")),
                declared_size: Some(0),
                data: data_complete.then(|| SpooledBlob {
                    sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                        .to_owned(),
                    byte_len: 0,
                    pack_offset: Some(0),
                }),
                data_complete,
                properties: Vec::new(),
                embedded: None,
            }
        }

        assert!(!document_message_class("IPM.Document"));
        assert!(document_message_class("IPM.Document.Word.Document.12"));
        assert!(document_message_class("ipm.document.txtfile"));
        assert!(!document_message_class("IPM.Documentary"));

        let directory = tempdir()?;
        let job = DurableCatalogSink::create(&directory.path().join("job"))?;
        let document = mail("IPM.Document.Word.Document.12", "recovered:-:0:0");
        let input = build_part_writer_input(
            &job,
            &[&document],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(input.partial);
        assert_eq!(
            input
                .reconstructions
                .generated
                .get(&ReconstructedField::DocumentAttachment),
            Some(&1)
        );

        let root = mail("IPM.Document", "recovered:-:0:1");
        let root_input = build_part_writer_input(
            &job,
            &[&root],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(!root_input.partial);
        assert!(
            !root_input
                .reconstructions
                .generated
                .contains_key(&ReconstructedField::DocumentAttachment)
        );

        let mut multiple = mail("IPM.Document.Word.Document.12", "recovered:-:0:2");
        multiple.attachments = vec![attachment(0, true), attachment(1, true)];
        let multiple_input = build_part_writer_input(
            &job,
            &[&multiple],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(!multiple_input.partial);
        assert_eq!(
            multiple_input.store.folders[0].messages[0]
                .attachments
                .len(),
            2
        );
        assert!(
            !multiple_input
                .reconstructions
                .generated
                .contains_key(&ReconstructedField::DocumentAttachment)
        );

        let mut unreadable = mail("IPM.Document.Word.Document.12", "recovered:-:0:3");
        unreadable.attachments = vec![attachment(0, false)];
        let unreadable_input = build_part_writer_input(
            &job,
            &[&unreadable],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(unreadable_input.partial);
        assert_eq!(unreadable_input.omitted_attachments, 1);
        assert!(
            unreadable_input.store.folders[0].messages[0]
                .attachments
                .is_empty()
        );
        assert_eq!(
            unreadable_input
                .reconstructions
                .generated
                .get(&ReconstructedField::DocumentAttachment),
            Some(&1)
        );
        Ok(())
    }

    #[test]
    fn distribution_list_containment_keeps_primary_members() {
        let property = |lid, value| NamedProperty {
            set: NamedPropertySet::Guid(super::PSETID_ADDRESS),
            name: NamedPropertyName::Numeric(lid),
            value,
        };
        let mut properties = vec![
            property(
                0x8055,
                RawPropertyValue::MultipleBinary(vec![vec![1], vec![2]]),
            ),
            property(0x8054, RawPropertyValue::MultipleBinary(vec![vec![1]])),
            property(0x804C, RawPropertyValue::Integer32(7)),
        ];
        let mut partial = false;
        let mut omitted = 0;
        contain_distribution_list_properties(&mut properties, &mut partial, &mut omitted);
        assert!(partial);
        assert_eq!(omitted, 1);
        assert!(
            properties
                .iter()
                .any(|property| property.name == NamedPropertyName::Numeric(0x8055))
        );
        assert!(
            !properties
                .iter()
                .any(|property| property.name == NamedPropertyName::Numeric(0x8054))
        );
        assert!(
            properties
                .iter()
                .any(|property| property.name == NamedPropertyName::Numeric(0x804C))
        );

        let mut orphaned = vec![
            property(0x8054, RawPropertyValue::MultipleBinary(vec![vec![1]])),
            property(0x804C, RawPropertyValue::Integer32(7)),
        ];
        partial = false;
        omitted = 0;
        contain_distribution_list_properties(&mut orphaned, &mut partial, &mut omitted);
        assert!(partial);
        assert_eq!(omitted, 2);
        assert!(orphaned.is_empty());

        let mut malformed = vec![
            property(0x8055, RawPropertyValue::Binary(vec![1])),
            property(0x8054, RawPropertyValue::MultipleBinary(vec![vec![1]])),
            property(0x804C, RawPropertyValue::Unicode("wrong".to_owned())),
        ];
        partial = false;
        omitted = 0;
        contain_distribution_list_properties(&mut malformed, &mut partial, &mut omitted);
        assert!(partial);
        assert_eq!(omitted, 3);
        assert!(malformed.is_empty());

        assert_eq!(default_container_class("IPM.DistList"), "IPF.Contact");
        assert_eq!(
            default_container_class("ipm.distlist.recovered"),
            "IPF.Contact"
        );
    }

    #[test]
    fn distribution_list_translation_contains_bad_arrays_once_and_keeps_valid_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        fn send(
            sink: &mut DurableCatalogSink,
            event: CatalogEvent<'_>,
        ) -> Result<(), std::io::Error> {
            CatalogSink::event(sink, event).map_err(std::io::Error::other)
        }

        fn encoded_single_value(total_bytes: usize) -> Vec<u8> {
            let mut encoded = Vec::with_capacity(total_bytes);
            encoded.extend_from_slice(&1_u32.to_le_bytes());
            encoded.extend_from_slice(&8_u32.to_le_bytes());
            encoded.resize(total_bytes, 0xA5);
            encoded
        }

        fn mail(blob: SpooledBlob, value_type: u32, key: &str) -> CanonicalMail {
            let spooled_bytes = blob.byte_len;
            CanonicalMail {
                durable_item_key: key.to_owned(),
                key: ItemKey {
                    provenance: RecoveryProvenance::Recovered,
                    source_node_id: None,
                    recovery_index: Some(0),
                    occurrence: 0,
                },
                folder_path: vec!["Contacts".to_owned()],
                folder_location: CanonicalFolderLocation::IpmSubtree,
                folder_role: CanonicalFolderRole::Ordinary,
                placement: CanonicalMessagePlacement::Normal,
                message_class: Some("IPM.DistList".to_owned()),
                subject: Some("Distribution-list containment checkpoint".to_owned()),
                sender_name: None,
                sender_email: None,
                submit_filetime: None,
                delivery_filetime: None,
                recipients: Vec::new(),
                attachments: Vec::new(),
                properties: vec![CanonicalProperty {
                    owner: "message".to_owned(),
                    owner_index: None,
                    record_set_index: 0,
                    entry_index: 0,
                    property_id: Some(0x8000),
                    value_type: Some(value_type),
                    named_property: Some(NamedPropertyIdentity {
                        guid: super::PSETID_ADDRESS,
                        name: LibpffNamedPropertyName::Numeric(0x8055),
                    }),
                    blob,
                }],
                completeness: ContentCompleteness::Complete,
                spooled_bytes,
            }
        }

        let malformed = vec![1, 0, 0, 0, 7, 0, 0, 0];
        let oversized = encoded_single_value(15_000);
        let wrong_type = vec![0x3D, 0xD8];
        let maximum_valid = encoded_single_value(14_999);
        let payloads = [&malformed, &oversized, &wrong_type, &maximum_valid];
        let directory = tempdir()?;
        let mut job = DurableCatalogSink::create(&directory.path().join("job"))?;
        let unit = RecoveryUnit::Recovered { index: 0 };
        send(&mut job, CatalogEvent::UnitStart(unit))?;
        send(
            &mut job,
            CatalogEvent::MessageStart {
                id: 9,
                provenance: CatalogProvenance::Recovered,
                recovery_index: Some(0),
                folder_id: None,
                parent_message_id: None,
                parent_attachment_index: None,
                embedded_path: Vec::new(),
                associated: false,
                item_type: None,
                message_class: Some("IPM.DistList".to_owned()),
                subject: None,
                sender_name: None,
                sender_email: None,
                submit_filetime: None,
                delivery_filetime: None,
                supported: true,
            },
        )?;
        for (index, payload) in payloads.into_iter().enumerate() {
            let descriptor = PropertyDescriptor {
                owner: PropertyOwner::Message(9),
                record_set_index: 0,
                entry_index: u32::try_from(index)?,
                entry_type: Some(0x7000 + u32::try_from(index)?),
                value_type: Some(0x0102),
                data_size: u64::try_from(payload.len())?,
            };
            send(&mut job, CatalogEvent::PropertyStart(descriptor))?;
            send(
                &mut job,
                CatalogEvent::PropertyData {
                    descriptor,
                    bytes: payload,
                },
            )?;
            send(&mut job, CatalogEvent::PropertyEnd(descriptor))?;
        }
        send(
            &mut job,
            CatalogEvent::MessageEnd {
                id: 9,
                complete: true,
            },
        )?;
        send(&mut job, CatalogEvent::UnitEnd(unit))?;
        let blobs = job
            .spooled_candidates()?
            .into_iter()
            .flat_map(|candidate| candidate.events)
            .filter(|event| event.kind == "property")
            .filter_map(|event| event.blob)
            .collect::<Vec<_>>();
        assert_eq!(blobs.len(), 4);

        for (index, value_type) in [0x1102, 0x1102, 0x001F].into_iter().enumerate() {
            let candidate = mail(
                blobs[index].clone(),
                value_type,
                &format!("recovered:-:0:{index}"),
            );
            let input = build_part_writer_input(
                &job,
                &[&candidate],
                &"0".repeat(64),
                "balanced",
                4_294_967_296,
                1,
            )?;
            assert!(input.partial);
            assert_eq!(input.omitted_properties, 1);
            assert!(
                input.store.folders[0].messages[0]
                    .named_properties
                    .is_empty()
            );
        }

        let candidate = mail(blobs[3].clone(), 0x1102, "recovered:-:0:3");
        let input = build_part_writer_input(
            &job,
            &[&candidate],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(!input.partial);
        assert_eq!(input.omitted_properties, 0);
        assert_eq!(input.store.folders[0].container_class, "IPF.Contact");
        assert_eq!(
            input.store.folders[0].messages[0].named_properties,
            vec![NamedProperty {
                set: NamedPropertySet::Guid(super::PSETID_ADDRESS),
                name: NamedPropertyName::Numeric(0x8055),
                value: RawPropertyValue::MultipleBinary(vec![vec![0xA5; 14_991]]),
            }]
        );
        validate_mail_store_input(&input.store)?;
        Ok(())
    }

    #[test]
    fn missing_internet_codepage_is_derived_only_from_valid_utf8_html() {
        let mut valid_html = Default::default();
        record_internet_codepage_provenance(None, true, &mut valid_html);
        assert_eq!(
            valid_html
                .derived
                .get(&ReconstructedField::InternetCodepage),
            Some(&1)
        );
        assert!(valid_html.generated.is_empty());

        let mut ambiguous = Default::default();
        record_internet_codepage_provenance(None, false, &mut ambiguous);
        assert_eq!(
            ambiguous
                .generated
                .get(&ReconstructedField::InternetCodepage),
            Some(&1)
        );
        assert!(ambiguous.derived.is_empty());

        let mut source_value = Default::default();
        record_internet_codepage_provenance(Some(1252), false, &mut source_value);
        assert!(source_value.is_empty());
    }

    #[test]
    fn attachment_mime_signatures_are_narrow_and_unambiguous() {
        for (bytes, expected) in [
            (&b"%PDF-1.7"[..], "application/pdf"),
            (&b"\x89PNG\r\n\x1a\nrest"[..], "image/png"),
            (&b"\xFF\xD8\xFF\xE0\x00\x10JFIF\0"[..], "image/jpeg"),
            (&b"GIF87arest"[..], "image/gif"),
            (&b"GIF89arest"[..], "image/gif"),
            (&b"II*\0rest"[..], "image/tiff"),
            (&b"MM\0*rest"[..], "image/tiff"),
        ] {
            assert_eq!(mime_from_signature(bytes), Some(expected));
        }
        for ambiguous in [
            &b""[..],
            &b"%PDF"[..],
            &b"\xFF\xD8\xFF"[..],
            &b"\xFF\xD8\xFF\xE1invalid"[..],
            &b"PK\x03\x04"[..],
            &b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1"[..],
            &b"plain text"[..],
        ] {
            assert_eq!(mime_from_signature(ambiguous), None);
        }
    }

    #[test]
    fn generated_attachment_extensions_require_recognized_mime() {
        for (mime, expected) in [
            ("application/pdf", "pdf"),
            ("IMAGE/JPEG; name=checkpoint", "jpg"),
            ("application/zip", "zip"),
            (
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "docx",
            ),
            ("application/vnd.ms-excel", "xls"),
        ] {
            assert_eq!(extension_for_mime(mime), Some(expected));
        }
        for ambiguous in [
            "",
            "application/octet-stream",
            "text/plain",
            "application/x-owner",
        ] {
            assert_eq!(extension_for_mime(ambiguous), None);
        }
    }

    #[test]
    fn missing_attachment_mime_is_derived_from_verified_payload_prefix()
    -> Result<(), Box<dyn std::error::Error>> {
        fn send(
            sink: &mut DurableCatalogSink,
            event: CatalogEvent<'_>,
        ) -> Result<(), std::io::Error> {
            CatalogSink::event(sink, event).map_err(std::io::Error::other)
        }

        let directory = tempdir()?;
        let mut job = DurableCatalogSink::create(&directory.path().join("job"))?;
        let unit = RecoveryUnit::Recovered { index: 0 };
        let payload = b"%PDF-1.7\ncheckpoint";
        let truncated_jpeg = b"\xFF\xD8\xFF";
        let source_mime = "application/x-owner\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let docx_mime = "application/vnd.openxmlformats-officedocument.wordprocessingml.document\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        send(&mut job, CatalogEvent::UnitStart(unit))?;
        send(
            &mut job,
            CatalogEvent::MessageStart {
                id: 7,
                provenance: CatalogProvenance::Recovered,
                recovery_index: Some(0),
                folder_id: None,
                parent_message_id: None,
                parent_attachment_index: None,
                embedded_path: Vec::new(),
                associated: false,
                item_type: None,
                message_class: Some("IPM.Note".to_owned()),
                subject: Some("MIME signature checkpoint".to_owned()),
                sender_name: Some("PSTForge".to_owned()),
                sender_email: Some("sender@example.com".to_owned()),
                submit_filetime: Some(1),
                delivery_filetime: Some(1),
                supported: true,
            },
        )?;
        send(
            &mut job,
            CatalogEvent::AttachmentStart {
                message_id: 7,
                index: 0,
                attachment_type: Some(i32::from(b'd')),
                data_size: Some(u64::try_from(payload.len())?),
                filename: Some("checkpoint.pdf".to_owned()),
            },
        )?;
        let mime_descriptor = PropertyDescriptor {
            owner: PropertyOwner::Attachment {
                message_id: 7,
                index: 0,
            },
            record_set_index: 0,
            entry_index: 0,
            entry_type: Some(0x370E),
            value_type: Some(0x001F),
            data_size: u64::try_from(source_mime.len())?,
        };
        send(&mut job, CatalogEvent::PropertyStart(mime_descriptor))?;
        send(
            &mut job,
            CatalogEvent::PropertyData {
                descriptor: mime_descriptor,
                bytes: &source_mime,
            },
        )?;
        send(&mut job, CatalogEvent::PropertyEnd(mime_descriptor))?;
        send(
            &mut job,
            CatalogEvent::AttachmentData {
                message_id: 7,
                index: 0,
                bytes: payload,
            },
        )?;
        send(
            &mut job,
            CatalogEvent::AttachmentEnd {
                message_id: 7,
                index: 0,
            },
        )?;
        send(
            &mut job,
            CatalogEvent::AttachmentStart {
                message_id: 7,
                index: 1,
                attachment_type: Some(i32::from(b'd')),
                data_size: Some(u64::try_from(truncated_jpeg.len())?),
                filename: Some("truncated.jpg".to_owned()),
            },
        )?;
        send(
            &mut job,
            CatalogEvent::AttachmentData {
                message_id: 7,
                index: 1,
                bytes: truncated_jpeg,
            },
        )?;
        send(
            &mut job,
            CatalogEvent::AttachmentEnd {
                message_id: 7,
                index: 1,
            },
        )?;
        send(
            &mut job,
            CatalogEvent::AttachmentStart {
                message_id: 7,
                index: 2,
                attachment_type: Some(i32::from(b'd')),
                data_size: Some(u64::try_from(truncated_jpeg.len())?),
                filename: None,
            },
        )?;
        let docx_mime_descriptor = PropertyDescriptor {
            owner: PropertyOwner::Attachment {
                message_id: 7,
                index: 2,
            },
            record_set_index: 0,
            entry_index: 0,
            entry_type: Some(0x370E),
            value_type: Some(0x001F),
            data_size: u64::try_from(docx_mime.len())?,
        };
        send(&mut job, CatalogEvent::PropertyStart(docx_mime_descriptor))?;
        send(
            &mut job,
            CatalogEvent::PropertyData {
                descriptor: docx_mime_descriptor,
                bytes: &docx_mime,
            },
        )?;
        send(&mut job, CatalogEvent::PropertyEnd(docx_mime_descriptor))?;
        send(
            &mut job,
            CatalogEvent::AttachmentData {
                message_id: 7,
                index: 2,
                bytes: truncated_jpeg,
            },
        )?;
        send(
            &mut job,
            CatalogEvent::AttachmentEnd {
                message_id: 7,
                index: 2,
            },
        )?;
        send(
            &mut job,
            CatalogEvent::MessageEnd {
                id: 7,
                complete: true,
            },
        )?;
        send(&mut job, CatalogEvent::UnitEnd(unit))?;
        let events = job
            .spooled_candidates()?
            .into_iter()
            .flat_map(|candidate| candidate.events)
            .collect::<Vec<_>>();
        let mut attachment_blobs = events
            .iter()
            .filter(|event| event.kind == "attachment_data")
            .filter_map(|event| event.blob.clone());
        let blob = attachment_blobs
            .next()
            .ok_or("missing attachment payload blob")?;
        let truncated_blob = attachment_blobs
            .next()
            .ok_or("missing truncated attachment payload blob")?;
        let source_mime_blob = events
            .iter()
            .filter(|event| event.kind == "property")
            .filter_map(|event| event.blob.clone())
            .next()
            .ok_or("missing source attachment MIME blob")?;
        let docx_mime_blob = events
            .iter()
            .filter(|event| event.kind == "property")
            .filter_map(|event| event.blob.clone())
            .nth(1)
            .ok_or("missing DOCX attachment MIME blob")?;
        let docx_payload_blob = attachment_blobs
            .next()
            .ok_or("missing DOCX attachment payload blob")?;
        let mail = CanonicalMail {
            durable_item_key: "recovered:-:0:0".to_owned(),
            key: ItemKey {
                provenance: RecoveryProvenance::Recovered,
                source_node_id: None,
                recovery_index: Some(0),
                occurrence: 0,
            },
            folder_path: vec!["Recovered".to_owned()],
            folder_location: CanonicalFolderLocation::IpmSubtree,
            folder_role: CanonicalFolderRole::Ordinary,
            placement: CanonicalMessagePlacement::Normal,
            message_class: Some("IPM.Note".to_owned()),
            subject: Some("MIME signature checkpoint".to_owned()),
            sender_name: Some("PSTForge".to_owned()),
            sender_email: Some("sender@example.com".to_owned()),
            submit_filetime: Some(1),
            delivery_filetime: Some(1),
            recipients: Vec::new(),
            attachments: vec![
                CanonicalAttachment {
                    index: 0,
                    attachment_type: Some(i32::from(b'd')),
                    filename: None,
                    declared_size: Some(u64::try_from(payload.len())?),
                    data: Some(blob),
                    data_complete: true,
                    properties: Vec::new(),
                    embedded: None,
                },
                CanonicalAttachment {
                    index: 1,
                    attachment_type: Some(i32::from(b'd')),
                    filename: None,
                    declared_size: Some(u64::try_from(truncated_jpeg.len())?),
                    data: Some(truncated_blob),
                    data_complete: true,
                    properties: Vec::new(),
                    embedded: None,
                },
                CanonicalAttachment {
                    index: 2,
                    attachment_type: Some(i32::from(b'd')),
                    filename: None,
                    declared_size: Some(u64::try_from(truncated_jpeg.len())?),
                    data: Some(docx_payload_blob),
                    data_complete: true,
                    properties: vec![CanonicalProperty {
                        owner: "attachment".to_owned(),
                        owner_index: Some(2),
                        record_set_index: 0,
                        entry_index: 0,
                        property_id: Some(0x370E),
                        value_type: Some(0x001F),
                        named_property: None,
                        blob: docx_mime_blob,
                    }],
                    embedded: None,
                },
            ],
            properties: Vec::new(),
            completeness: ContentCompleteness::Complete,
            spooled_bytes: u64::try_from(payload.len())?,
        };

        let input = build_part_writer_input(
            &job,
            &[&mail],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        let attachment = &input.store.folders[0].messages[0].attachments[0];
        assert_eq!(attachment.mime_type.as_deref(), Some("application/pdf"));
        assert_eq!(attachment.filename, "Recovered attachment 0.pdf");
        assert_eq!(attachment.rendering_position, None);
        assert_eq!(attachment.flags, 0);
        assert_eq!(
            input.store.folders[0].messages[0].attachments[1].mime_type,
            None
        );
        assert_eq!(
            input.store.folders[0].messages[0].attachments[1].filename,
            "Recovered attachment 1.bin"
        );
        assert_eq!(
            input.store.folders[0].messages[0].attachments[2]
                .mime_type
                .as_deref(),
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        );
        assert_eq!(
            input.store.folders[0].messages[0].attachments[2].filename,
            "Recovered attachment 2.docx"
        );
        assert_eq!(
            input
                .reconstructions
                .derived
                .get(&ReconstructedField::AttachmentMimeType),
            Some(&1)
        );
        assert_eq!(
            input
                .reconstructions
                .generated
                .get(&ReconstructedField::AttachmentRenderingPosition),
            Some(&3)
        );
        assert_eq!(
            input
                .reconstructions
                .generated
                .get(&ReconstructedField::AttachmentFlags),
            Some(&3)
        );
        assert_eq!(
            input
                .reconstructions
                .generated
                .get(&ReconstructedField::AttachmentFilename),
            Some(&3)
        );
        assert!(!input.partial);
        let neutral_defaults_pst = directory.path().join("neutral-attachment-defaults.pst");
        pstforge_pst::writer::create_mail_store(&neutral_defaults_pst, &input.store)?;
        assert!(neutral_defaults_pst.is_file());

        let mut source_wins = mail;
        source_wins.attachments[0]
            .properties
            .push(CanonicalProperty {
                owner: "attachment".to_owned(),
                owner_index: Some(0),
                record_set_index: 0,
                entry_index: 0,
                property_id: Some(0x370E),
                value_type: Some(0x001F),
                named_property: None,
                blob: source_mime_blob,
            });
        let source_input = build_part_writer_input(
            &job,
            &[&source_wins],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        let attachment = &source_input.store.folders[0].messages[0].attachments[0];
        assert_eq!(attachment.mime_type.as_deref(), Some("application/x-owner"));
        assert_eq!(attachment.filename, "Recovered attachment 0.pdf");
        assert!(
            !source_input
                .reconstructions
                .derived
                .contains_key(&ReconstructedField::AttachmentMimeType)
        );
        Ok(())
    }

    #[test]
    fn normalizes_message_role_to_retained_same_path_folder()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = DurableCatalogSink::create(&directory.path().join("job"))?;
        let mail = CanonicalMail {
            durable_item_key: "normal:1:-:0".to_owned(),
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(1),
                recovery_index: None,
                occurrence: 0,
            },
            folder_path: vec!["Deleted Items".to_owned()],
            folder_location: CanonicalFolderLocation::IpmSubtree,
            folder_role: CanonicalFolderRole::Ordinary,
            placement: CanonicalMessagePlacement::Normal,
            message_class: Some("IPM.Note".to_owned()),
            subject: Some("colliding folder message".to_owned()),
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            recipients: Vec::new(),
            attachments: Vec::new(),
            properties: Vec::new(),
            completeness: ContentCompleteness::Complete,
            spooled_bytes: 0,
        };
        let retained = [CanonicalFolder {
            path: vec!["Deleted Items".to_owned()],
            location: CanonicalFolderLocation::IpmSubtree,
            role: CanonicalFolderRole::DeletedItems,
            container_class: Some("IPF.Note".to_owned()),
        }];
        let interrupted = AtomicBool::new(false);
        let input = build_part_writer_input_with_folders_interruptible(
            &job,
            &[&mail],
            &retained,
            PartBuildOptions {
                source_sha256: &"0".repeat(64),
                recovery_mode: "balanced",
                maximum_pst_bytes: 4_294_967_296,
                part_index: 1,
                omitted_folders: 1,
            },
            &interrupted,
        )?;

        assert_eq!(input.store.folders.len(), 1);
        assert_eq!(input.store.folders[0].role, MailFolderRole::DeletedItems);
        assert_eq!(input.store.folders[0].messages.len(), 1);
        assert!(input.partial);
        assert_eq!(input.omitted_folders, 1);
        Ok(())
    }

    #[test]
    fn preserves_source_contact_class_and_derives_one_sided_contact_sender()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = DurableCatalogSink::create(&directory.path().join("job"))?;
        let mail = CanonicalMail {
            durable_item_key: "normal:2:-:0".to_owned(),
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(2),
                recovery_index: None,
                occurrence: 0,
            },
            folder_path: vec!["Contacts".to_owned()],
            folder_location: CanonicalFolderLocation::IpmSubtree,
            folder_role: CanonicalFolderRole::Ordinary,
            placement: CanonicalMessagePlacement::Normal,
            message_class: Some("IPM.Contact".to_owned()),
            subject: Some("Ada Lovelace".to_owned()),
            sender_name: Some("not valid contact sender metadata".to_owned()),
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            recipients: Vec::new(),
            attachments: Vec::new(),
            properties: Vec::new(),
            completeness: ContentCompleteness::Complete,
            spooled_bytes: 0,
        };
        let retained = [CanonicalFolder {
            path: vec!["Contacts".to_owned()],
            location: CanonicalFolderLocation::IpmSubtree,
            role: CanonicalFolderRole::Ordinary,
            container_class: Some("IPF.Contact".to_owned()),
        }];
        let input = build_part_writer_input_with_folders_interruptible(
            &job,
            &[&mail],
            &retained,
            PartBuildOptions {
                source_sha256: &"0".repeat(64),
                recovery_mode: "balanced",
                maximum_pst_bytes: 4_294_967_296,
                part_index: 2,
                omitted_folders: 0,
            },
            &AtomicBool::new(false),
        )?;

        assert_eq!(input.store.folders[0].container_class, "IPF.Contact");
        assert_eq!(
            input.store.folders[0].messages[0].sender_name,
            "not valid contact sender metadata"
        );
        assert_eq!(
            input.store.folders[0].messages[0].sender_email,
            "not valid contact sender metadata"
        );
        assert_eq!(
            input
                .reconstructions
                .derived
                .get(&ReconstructedField::SenderAddress),
            Some(&1)
        );
        assert_eq!(input.omitted_properties, 0);
        assert!(!input.partial);
        validate_mail_store_input(&input.store)?;
        Ok(())
    }

    #[test]
    fn preserves_senderless_appointment_in_source_calendar()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = DurableCatalogSink::create(&directory.path().join("job"))?;
        let mail = CanonicalMail {
            durable_item_key: "normal:3:-:0".to_owned(),
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(3),
                recovery_index: None,
                occurrence: 0,
            },
            folder_path: vec!["Calendar".to_owned()],
            folder_location: CanonicalFolderLocation::IpmSubtree,
            folder_role: CanonicalFolderRole::Ordinary,
            placement: CanonicalMessagePlacement::Normal,
            message_class: Some("IPM.Appointment".to_owned()),
            subject: Some("Appointment fidelity checkpoint".to_owned()),
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            recipients: Vec::new(),
            attachments: Vec::new(),
            properties: Vec::new(),
            completeness: ContentCompleteness::Complete,
            spooled_bytes: 0,
        };
        let retained = [CanonicalFolder {
            path: vec!["Calendar".to_owned()],
            location: CanonicalFolderLocation::IpmSubtree,
            role: CanonicalFolderRole::Ordinary,
            container_class: Some("IPF.Appointment".to_owned()),
        }];
        let input = build_part_writer_input_with_folders_interruptible(
            &job,
            &[&mail],
            &retained,
            PartBuildOptions {
                source_sha256: &"0".repeat(64),
                recovery_mode: "balanced",
                maximum_pst_bytes: 4_294_967_296,
                part_index: 1,
                omitted_folders: 0,
            },
            &AtomicBool::new(false),
        )?;

        assert_eq!(input.store.folders[0].container_class, "IPF.Appointment");
        assert_eq!(input.store.folders[0].messages[0].sender_name, "");
        assert_eq!(input.store.folders[0].messages[0].sender_email, "");
        assert_eq!(input.omitted_properties, 0);
        assert!(!input.partial);
        assert!(input.reconstructions.derived.is_empty());
        for field in [
            ReconstructedField::MessageFlags,
            ReconstructedField::InternetCodepage,
            ReconstructedField::SubmitTime,
            ReconstructedField::DeliveryTime,
            ReconstructedField::CreationTime,
            ReconstructedField::ModificationTime,
        ] {
            assert_eq!(input.reconstructions.generated.get(&field), Some(&1));
        }
        assert_eq!(input.reconstructions.generated.len(), 6);
        validate_mail_store_input(&input.store)?;
        Ok(())
    }

    #[test]
    fn missing_mail_metadata_is_accounted_without_marking_source_loss()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = DurableCatalogSink::create(&directory.path().join("job"))?;
        let mail = CanonicalMail {
            durable_item_key: "normal:4:-:0".to_owned(),
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(4),
                recovery_index: None,
                occurrence: 0,
            },
            folder_path: vec!["Inbox".to_owned()],
            folder_location: CanonicalFolderLocation::IpmSubtree,
            folder_role: CanonicalFolderRole::Ordinary,
            placement: CanonicalMessagePlacement::Normal,
            message_class: Some("IPM.Note".to_owned()),
            subject: None,
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            recipients: Vec::new(),
            attachments: Vec::new(),
            properties: vec![CanonicalProperty {
                owner: "message".to_owned(),
                owner_index: None,
                record_set_index: 0,
                entry_index: 0,
                property_id: Some(0x1013),
                value_type: Some(0x0102),
                named_property: None,
                blob: SpooledBlob {
                    sha256: "0".repeat(64),
                    byte_len: 0,
                    pack_offset: None,
                },
            }],
            completeness: ContentCompleteness::Complete,
            spooled_bytes: 0,
        };
        let input = build_part_writer_input(
            &job,
            &[&mail],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;

        let output = &input.store.folders[0].messages[0];
        assert!(output.subject.is_empty());
        assert!(output.sender_name.is_empty());
        assert!(output.sender_email.is_empty());
        assert!(!input.partial);
        assert_eq!(
            input
                .reconstructions
                .derived
                .get(&ReconstructedField::FolderClass),
            Some(&1)
        );
        assert_eq!(input.reconstructions.derived.len(), 1);
        for field in [
            ReconstructedField::Subject,
            ReconstructedField::SenderName,
            ReconstructedField::SenderAddress,
            ReconstructedField::MessageFlags,
            ReconstructedField::InternetCodepage,
            ReconstructedField::SubmitTime,
            ReconstructedField::DeliveryTime,
            ReconstructedField::CreationTime,
            ReconstructedField::ModificationTime,
        ] {
            assert_eq!(input.reconstructions.generated.get(&field), Some(&1));
        }
        assert_eq!(input.reconstructions.generated.len(), 9);

        let mut associated = mail.clone();
        associated.durable_item_key = "associated:4:-:0".to_owned();
        associated.placement = CanonicalMessagePlacement::Associated;
        associated.properties.clear();
        let associated_input = build_part_writer_input(
            &job,
            &[&associated],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            2,
        )?;
        let associated_output = &associated_input.store.folders[0].associated_messages[0];
        assert!(associated_output.subject.is_empty());
        assert!(associated_output.raw_properties.iter().any(|property| {
            matches!(
                property,
                pstforge_pst::writer::RawProperty {
                    id: 0x3001,
                    value: pstforge_pst::writer::RawPropertyValue::Unicode(value),
                } if value == "(no subject)"
            )
        }));
        assert_eq!(
            associated_input
                .reconstructions
                .generated
                .get(&ReconstructedField::AssociatedDisplayName),
            Some(&1)
        );
        validate_mail_store_input(&associated_input.store)?;

        let mut empty_display = associated_output.clone();
        empty_display.subject = "Readable associated subject".to_owned();
        for property in &mut empty_display.raw_properties {
            if let pstforge_pst::writer::RawProperty {
                id: 0x3001,
                value: pstforge_pst::writer::RawPropertyValue::Unicode(value),
            } = property
            {
                value.clear();
            }
        }
        let mut display_counts = ReconstructionCounts::default();
        normalize_associated_display_name(&mut empty_display, &mut display_counts);
        assert!(empty_display.raw_properties.iter().any(|property| {
            matches!(
                property,
                pstforge_pst::writer::RawProperty {
                    id: 0x3001,
                    value: pstforge_pst::writer::RawPropertyValue::Unicode(value),
                } if value == "Readable associated subject"
            )
        }));
        assert_eq!(
            display_counts
                .derived
                .get(&ReconstructedField::AssociatedDisplayName),
            Some(&1)
        );
        assert!(display_counts.generated.is_empty());
        Ok(())
    }

    #[test]
    fn attachment_larger_than_writer_field_is_omitted_without_losing_parent()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = DurableCatalogSink::create(&directory.path().join("job"))?;
        let mail = CanonicalMail {
            durable_item_key: "normal:1:-:0".to_owned(),
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(1),
                recovery_index: None,
                occurrence: 0,
            },
            folder_path: vec!["Inbox".to_owned()],
            folder_location: CanonicalFolderLocation::IpmSubtree,
            folder_role: CanonicalFolderRole::Ordinary,
            placement: CanonicalMessagePlacement::Normal,
            message_class: Some("IPM.Note".to_owned()),
            subject: Some("large attachment".to_owned()),
            sender_name: Some("Sender".to_owned()),
            sender_email: Some("sender@example.com".to_owned()),
            submit_filetime: Some(1),
            delivery_filetime: Some(1),
            recipients: Vec::new(),
            attachments: vec![CanonicalAttachment {
                index: 0,
                attachment_type: Some(1),
                filename: Some("huge.bin".to_owned()),
                declared_size: Some(2_147_483_648),
                data: Some(SpooledBlob {
                    sha256: "0".repeat(64),
                    byte_len: 2_147_483_648,
                    pack_offset: None,
                }),
                data_complete: true,
                properties: Vec::new(),
                embedded: None,
            }],
            properties: Vec::new(),
            completeness: ContentCompleteness::Complete,
            spooled_bytes: 2_147_483_648,
        };
        let input = build_part_writer_input(
            &job,
            &[&mail],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(input.partial);
        assert_eq!(input.omitted_attachments, 1);
        assert!(input.store.folders[0].messages[0].attachments.is_empty());
        assert_eq!(input.item_keys, ["normal:1:-:0"]);
        let mut truncated = mail.clone();
        truncated.attachments[0].declared_size = Some(4);
        truncated.attachments[0].data = Some(SpooledBlob {
            sha256: "0".repeat(64),
            byte_len: 2,
            pack_offset: None,
        });
        truncated.attachments[0].data_complete = false;
        let truncated_input = build_part_writer_input(
            &job,
            &[&truncated],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(truncated_input.partial);
        assert_eq!(truncated_input.omitted_attachments, 1);
        assert!(
            truncated_input.store.folders[0].messages[0]
                .attachments
                .is_empty()
        );
        let mut empty = mail.clone();
        empty.attachments[0].declared_size = Some(0);
        empty.attachments[0].data = Some(SpooledBlob {
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned(),
            byte_len: 0,
            pack_offset: Some(0),
        });
        let empty_input = build_part_writer_input(
            &job,
            &[&empty],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(matches!(
            &empty_input.store.folders[0].messages[0].attachments[0].content,
            AttachmentContent::Binary(data) if data.is_empty()
        ));
        let aggressive = build_part_writer_input(
            &job,
            &[&mail],
            &"0".repeat(64),
            "aggressive",
            4_294_967_296,
            1,
        )?;
        let smaller = build_part_writer_input(
            &job,
            &[&mail],
            &"0".repeat(64),
            "balanced",
            4_000_000_000,
            1,
        )?;
        assert_ne!(input.store.record_key, aggressive.store.record_key);
        assert_ne!(input.store.record_key, smaller.store.record_key);
        Ok(())
    }

    #[test]
    fn unsupported_and_oversized_properties_are_omitted_once_before_writing()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = DurableCatalogSink::create(&directory.path().join("job"))?;
        let property = |entry_index, property_id, value_type, byte_len| CanonicalProperty {
            owner: "message".to_owned(),
            owner_index: None,
            record_set_index: 0,
            entry_index,
            property_id: Some(property_id),
            value_type: Some(value_type),
            named_property: None,
            blob: SpooledBlob {
                sha256: "0".repeat(64),
                byte_len,
                pack_offset: None,
            },
        };
        let mail = CanonicalMail {
            durable_item_key: "normal:1:-:0".to_owned(),
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(1),
                recovery_index: None,
                occurrence: 0,
            },
            folder_path: vec!["Inbox".to_owned()],
            folder_location: CanonicalFolderLocation::IpmSubtree,
            folder_role: CanonicalFolderRole::Ordinary,
            placement: CanonicalMessagePlacement::Normal,
            message_class: Some("IPM.Note".to_owned()),
            subject: Some("contained properties".to_owned()),
            sender_name: Some("Sender".to_owned()),
            sender_email: Some("sender@example.com".to_owned()),
            submit_filetime: Some(1),
            delivery_filetime: Some(1),
            recipients: Vec::new(),
            attachments: Vec::new(),
            properties: vec![
                property(0, 0x6001, 0x7777, 1),
                property(1, 0x6002, 0x0102, i32::MAX as u64 + 1),
                property(2, 0, 0x0003, 4),
                property(3, 0x0017, 0x0002, 2),
            ],
            completeness: ContentCompleteness::Complete,
            spooled_bytes: i32::MAX as u64 + 6,
        };

        let input = build_part_writer_input(
            &job,
            &[&mail],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        let message = &input.store.folders[0].messages[0];
        assert!(input.partial);
        assert_eq!(input.omitted_properties, 4);
        assert_eq!(message.unsupported_properties.len(), 4);
        assert!(
            message
                .unsupported_properties
                .iter()
                .any(|property| property.id == 0x0017 && property.property_type == 0x0002)
        );
        assert!(
            message
                .raw_properties
                .iter()
                .all(|property| property.id != 0x0017)
        );
        assert!(message.spooled_properties.is_empty());
        assert!(writer_stream_type_is_supported(0x0102));
        assert!(!writer_stream_type_is_supported(0x1003));
        assert!(!writer_stream_type_is_supported(0x7777));

        let mut native_body = Some(NativeBody::Rtf);
        let mut rtf_in_sync = true;
        let mut body_partial = false;
        let mut body_omissions = 0;
        contain_body_metadata(
            &mut native_body,
            &mut rtf_in_sync,
            true,
            &[],
            &mut body_partial,
            &mut body_omissions,
        );
        assert_eq!(native_body, None);
        assert!(!rtf_in_sync);
        assert!(body_partial);
        assert_eq!(body_omissions, 2);

        let mut false_sync = false;
        let mut no_native_body = None;
        let mut false_sync_partial = false;
        let mut false_sync_omissions = 0;
        contain_body_metadata(
            &mut no_native_body,
            &mut false_sync,
            true,
            &[],
            &mut false_sync_partial,
            &mut false_sync_omissions,
        );
        assert!(false_sync_partial);
        assert_eq!(false_sync_omissions, 1);

        assert!(valid_utf16_stream(&mut Cursor::new([
            0x3D, 0xD8, 0x00, 0xDE, 0, 0,
        ]))?);
        assert!(!valid_utf16_stream(&mut Cursor::new([0x3D, 0xD8, 0, 0]))?);
        assert!(valid_utf16_stream(&mut Cursor::new([b'A', 0]))?);
        assert!(valid_utf8_stream(
            &mut Cursor::new("chunk boundary €".as_bytes()),
            "test"
        )?);
        assert!(!valid_utf8_stream(
            &mut Cursor::new([0x66, 0x80, 0x6f]),
            "test"
        )?);

        let valid_rtf = compressed_rtf::compress_rtf("{\\rtf1 contained}")?;
        assert!(valid_compressed_rtf_container(&valid_rtf));
        let mut wrong_raw_size = valid_rtf;
        let declared = u32::from_le_bytes(wrong_raw_size[4..8].try_into()?);
        wrong_raw_size[4..8].copy_from_slice(&declared.saturating_add(1).to_le_bytes());
        assert!(!valid_compressed_rtf_container(&wrong_raw_size));

        let mut missing_end_run = compressed_rtf::compress_rtf("{\\rtf1 contained}")?;
        missing_end_run.truncate(missing_end_run.len().saturating_sub(2));
        rewrite_compressed_rtf_header(&mut missing_end_run)?;
        assert!(!valid_compressed_rtf_container(&missing_end_run));

        let mut uncompressed = compressed_rtf::encode_rtf("{\\rtf1 contained}")?;
        uncompressed[12..16].copy_from_slice(&1_u32.to_le_bytes());
        assert!(!valid_compressed_rtf_container(&uncompressed));
        Ok(())
    }

    fn rewrite_compressed_rtf_header(bytes: &mut [u8]) -> Result<(), Box<dyn std::error::Error>> {
        let compressed_size = u32::try_from(bytes.len().checked_sub(4).ok_or("short RTF")?)?;
        bytes[0..4].copy_from_slice(&compressed_size.to_le_bytes());
        let crc = compressed_rtf_crc(&bytes[16..]);
        bytes[12..16].copy_from_slice(&crc.to_le_bytes());
        Ok(())
    }

    fn compressed_rtf_crc(bytes: &[u8]) -> u32 {
        bytes.iter().copied().fold(0_u32, |mut crc, byte| {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 0 {
                    crc >> 1
                } else {
                    (crc >> 1) ^ 0xEDB8_8320
                };
            }
            crc
        })
    }

    #[test]
    fn reports_invalid_filetimes_and_clean_embedded_mail_are_contained()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(supported_message_class("ipm.note.custom"));
        assert!(supported_message_class("Report.IPM.Note.DR"));
        assert!(supported_message_class("IPM.Contact"));
        assert!(supported_message_class("ipm.contact.custom"));
        assert!(supported_message_class("IPM.Appointment"));
        assert!(supported_message_class("ipm.appointment.custom"));
        assert!(supported_message_class("IPM.Schedule.Meeting.Request"));
        assert!(supported_message_class("ipm.schedule.meeting.resp.pos"));
        assert!(supported_message_class("IPM.Task"));
        assert!(supported_message_class("ipm.task.custom"));
        assert!(supported_message_class("IPM.StickyNote"));
        assert!(supported_message_class("ipm.stickynote.custom"));
        assert!(supported_message_class("IPM.Post"));
        assert!(supported_message_class("ipm.post.custom"));
        assert!(supported_message_class(
            "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}"
        ));
        assert!(supported_message_class(
            "ipm.ole.class.{00061055-0000-0000-c000-000000000046}"
        ));
        assert!(supported_message_class("IPM.NoteCustom"));
        assert!(supported_message_class("IPM.ContactCustom"));
        assert!(supported_message_class("IPM.AppointmentCustom"));
        assert!(supported_message_class("IPM.Schedule.Meeting"));
        assert!(supported_message_class("IPM.Schedule.MeetingRequest"));
        assert!(supported_message_class("IPM.TaskRequest"));
        assert!(supported_message_class("IPM.StickyNoteCustom"));
        assert!(supported_message_class("IPM.PostCustom"));
        assert!(supported_message_class(
            "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}.Custom"
        ));
        assert!(supported_message_class(
            "IPM.OLE.CLASS.{00061056-0000-0000-C000-000000000046}"
        ));
        assert!(supported_message_class("REPORT.IPM.NoteDR"));
        assert!(!supported_message_class(""));
        assert!(attachment_property_type_is_preservable(0x7FFB, 0x0040));
        assert!(!attachment_property_type_is_preservable(0x7FFB, 0x000B));

        let directory = tempdir()?;
        let job = DurableCatalogSink::create(&directory.path().join("job"))?;
        let message = |id: u32, class: &str| CanonicalMail {
            durable_item_key: format!("normal:{id}:-:0"),
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(id),
                recovery_index: None,
                occurrence: 0,
            },
            folder_path: vec!["Inbox".to_owned()],
            folder_location: CanonicalFolderLocation::IpmSubtree,
            folder_role: CanonicalFolderRole::Ordinary,
            placement: CanonicalMessagePlacement::Normal,
            message_class: Some(class.to_owned()),
            subject: Some(format!("message {id}")),
            sender_name: Some("Sender".to_owned()),
            sender_email: Some("sender@example.com".to_owned()),
            submit_filetime: Some(1),
            delivery_filetime: Some(2),
            recipients: Vec::new(),
            attachments: Vec::new(),
            properties: Vec::new(),
            completeness: ContentCompleteness::Complete,
            spooled_bytes: 0,
        };
        let embedded = message(3, "IPM.Note");
        let mut report = message(2, "Report.IPM.Note.DR");
        report.attachments.push(CanonicalAttachment {
            index: 0,
            attachment_type: Some(5),
            filename: Some("original.msg".to_owned()),
            declared_size: None,
            data: None,
            data_complete: false,
            properties: [0x0E20, 0x3701, 0x3704, 0x3705, 0x3707]
                .into_iter()
                .enumerate()
                .map(|(entry_index, property_id)| CanonicalProperty {
                    owner: "attachment".to_owned(),
                    owner_index: Some(0),
                    record_set_index: 0,
                    entry_index: u32::try_from(entry_index).unwrap_or(u32::MAX),
                    property_id: Some(property_id),
                    value_type: None,
                    named_property: None,
                    blob: SpooledBlob {
                        sha256: "0".repeat(64),
                        byte_len: 0,
                        pack_offset: None,
                    },
                })
                .collect(),
            embedded: Some(Box::new(embedded)),
        });
        let clean = build_part_writer_input(
            &job,
            &[&report],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(!clean.partial);
        assert_eq!(clean.item_keys, ["normal:2:-:0", "normal:3:-:0"]);
        assert_eq!(
            clean
                .reconstructions
                .derived
                .get(&ReconstructedField::AttachmentMimeType),
            Some(&1)
        );
        assert!(
            !clean
                .reconstructions
                .generated
                .contains_key(&ReconstructedField::AttachmentMimeType)
        );
        pstforge_pst::writer::create_mail_store(
            directory.path().join("report-message.pst"),
            &clean.store,
        )?;

        let top_level_exception =
            message(4, "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}");
        assert!(matches!(
            build_part_writer_input(
                &job,
                &[&top_level_exception],
                &"0".repeat(64),
                "balanced",
                4_294_967_296,
                1,
            ),
            Err(CanonicalWriteError::InvalidCandidate { .. })
        ));

        let mut malformed_exception = report.clone();
        malformed_exception.message_class = Some("IPM.Appointment".to_owned());
        let exception = malformed_exception.attachments[0]
            .embedded
            .as_mut()
            .ok_or("missing exception child")?;
        exception.message_class =
            Some("IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}".to_owned());
        malformed_exception.attachments[0].properties = (0x7FFA..=0x7FFE)
            .enumerate()
            .map(|(entry_index, property_id)| CanonicalProperty {
                owner: "attachment".to_owned(),
                owner_index: Some(0),
                record_set_index: 0,
                entry_index: u32::try_from(entry_index).unwrap_or(u32::MAX),
                property_id: Some(property_id),
                value_type: None,
                named_property: None,
                blob: SpooledBlob {
                    sha256: "0".repeat(64),
                    byte_len: 0,
                    pack_offset: None,
                },
            })
            .collect();
        let mut duplicate_linkage = malformed_exception.attachments[0].properties[0].clone();
        duplicate_linkage.entry_index = 5;
        malformed_exception.attachments[0]
            .properties
            .push(duplicate_linkage);
        malformed_exception.attachments[0]
            .properties
            .push(CanonicalProperty {
                owner: "attachment".to_owned(),
                owner_index: Some(0),
                record_set_index: 0,
                entry_index: 6,
                property_id: Some(0x3709),
                value_type: Some(0x0102),
                named_property: None,
                blob: SpooledBlob {
                    sha256: "0".repeat(64),
                    byte_len: 1_048_577,
                    pack_offset: None,
                },
            });
        let contained_exception = build_part_writer_input(
            &job,
            &[&malformed_exception],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(contained_exception.partial);
        assert_eq!(contained_exception.item_keys, ["normal:2:-:0"]);
        assert_eq!(contained_exception.unsupported_item_keys, ["normal:3:-:0"]);
        assert_eq!(contained_exception.omitted_properties, 7);
        assert_eq!(contained_exception.omitted_attachments, 1);
        assert!(
            contained_exception.store.folders[0].messages[0]
                .attachments
                .is_empty()
        );
        pstforge_pst::writer::validate_mail_store_input(&contained_exception.store)?;

        let mut nested = report.clone();
        let nested_child = message(4, "IPM.Note");
        let first_embedded = nested.attachments[0]
            .embedded
            .as_mut()
            .ok_or("missing first embedded message")?;
        first_embedded.attachments.push(CanonicalAttachment {
            index: 1,
            attachment_type: Some(i32::from(b'i')),
            filename: Some("nested.msg".to_owned()),
            declared_size: None,
            data: None,
            data_complete: true,
            properties: Vec::new(),
            embedded: Some(Box::new(nested_child)),
        });
        let nested_input = build_part_writer_input(
            &job,
            &[&nested],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert_eq!(
            nested_input.item_keys,
            ["normal:2:-:0", "normal:3:-:0", "normal:4:-:0"]
        );
        assert!(nested_input.unsupported_item_keys.is_empty());
        assert!(!nested_input.partial);
        assert_eq!(nested_input.omitted_attachments, 0);

        let mut duplicate_attachment_properties = report.clone();
        let attachment = &mut duplicate_attachment_properties.attachments[0];
        let mut duplicate = attachment.properties[4].clone();
        duplicate.entry_index = 5;
        attachment.properties.push(duplicate);
        let mut alternate_record_set = attachment.properties[2].clone();
        alternate_record_set.record_set_index = 1;
        alternate_record_set.entry_index = 0;
        attachment.properties.push(alternate_record_set);
        let damaged_attachment = build_part_writer_input(
            &job,
            &[&duplicate_attachment_properties],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(damaged_attachment.partial);
        assert_eq!(damaged_attachment.omitted_properties, 2);

        report.submit_filetime = Some(u64::MAX);
        let malformed_time = build_part_writer_input(
            &job,
            &[&report],
            &"0".repeat(64),
            "balanced",
            4_294_967_296,
            1,
        )?;
        assert!(malformed_time.partial);
        assert_eq!(malformed_time.omitted_properties, 1);
        assert_eq!(malformed_time.store.folders[0].messages[0].sent_filetime, 0);

        std::thread::Builder::new()
            .name("deep-embedded-checkpoint".to_owned())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || -> Result<(), std::io::Error> {
                let mut child = message(356, "IPM.Note");
                for id in (100..356).rev() {
                    let mut parent = message(id, "IPM.Note");
                    parent.attachments.push(CanonicalAttachment {
                        index: 0,
                        attachment_type: Some(i32::from(b'i')),
                        filename: Some("nested.msg".to_owned()),
                        declared_size: None,
                        data: None,
                        data_complete: true,
                        properties: Vec::new(),
                        embedded: Some(Box::new(child)),
                    });
                    child = parent;
                }
                let deep_input = build_part_writer_input(
                    &job,
                    &[&child],
                    &"0".repeat(64),
                    "balanced",
                    4_294_967_296,
                    1,
                )
                .map_err(std::io::Error::other)?;
                assert_eq!(deep_input.item_keys.len(), 257);
                assert!(deep_input.unsupported_item_keys.is_empty());
                assert_eq!(deep_input.omitted_attachments, 0);
                assert!(!deep_input.partial);
                pstforge_pst::writer::create_mail_store(
                    directory.path().join("deep-nesting.pst"),
                    &deep_input.store,
                )
                .map_err(std::io::Error::other)?;
                Ok(())
            })?
            .join()
            .map_err(|_| std::io::Error::other("deep embedded checkpoint terminated"))??;
        Ok(())
    }

    #[test]
    fn recipient_table_schema_requires_exact_reconstructed_values() {
        let recipient = CanonicalRecipient {
            index: 0,
            recipient_type: Some(1),
            display_name: Some("Attendee".to_owned()),
            email_address: Some("attendee@example.com".to_owned()),
            address_type: Some("SMTP".to_owned()),
            properties: Vec::new(),
        };
        assert!(recipient_property_value_matches(
            &recipient,
            0x0FFE,
            &pstforge_pst::writer::RawPropertyValue::Integer32(6)
        ));
        assert!(recipient_property_value_matches(
            &recipient,
            0x39FF,
            &pstforge_pst::writer::RawPropertyValue::Unicode("attendee@example.com".to_owned())
        ));
        assert!(!recipient_property_value_matches(
            &recipient,
            0x0FFE,
            &pstforge_pst::writer::RawPropertyValue::Integer32(7)
        ));
        assert!(!recipient_property_value_matches(
            &recipient,
            0x0FF9,
            &pstforge_pst::writer::RawPropertyValue::Binary(vec![1])
        ));
        assert!(!recipient_property_value_matches(
            &recipient,
            0x39FE,
            &pstforge_pst::writer::RawPropertyValue::Unicode("legacy@example.com".to_owned())
        ));
    }
}
