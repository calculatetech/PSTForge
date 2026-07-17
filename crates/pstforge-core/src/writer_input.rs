use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

use pstforge_job::{DurableCatalogSink, JobError, SpooledBlob};
use pstforge_pst::writer::{
    AttachmentContent, AttachmentSpec, FileBlobSpec, MailFolderRole, MailFolderSpec, MailStoreSpec,
    MessageSpec, NativeBody, RawProperty, RawPropertyValue, RecipientKind, RecipientSpec,
    SpooledPropertySpec, UnsupportedProperty,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{CanonicalAttachment, CanonicalFolderRole, CanonicalMail, CanonicalProperty};

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
    pub omitted_properties: u64,
    pub omitted_attachments: u64,
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
        source_sha256,
        recovery_mode,
        maximum_pst_bytes,
        part_index,
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
        source_sha256,
        recovery_mode,
        maximum_pst_bytes,
        part_index,
        Some(interrupted),
    )
}

fn build_part_writer_input_expected(
    job: &DurableCatalogSink,
    messages: &[&CanonicalMail],
    source_sha256: &str,
    recovery_mode: &str,
    maximum_pst_bytes: u64,
    part_index: u32,
    interrupted: Option<&AtomicBool>,
) -> Result<PartWriterInput, CanonicalWriteError> {
    let source = TranslationSource { job, interrupted };
    source.check_interrupted()?;
    if messages.is_empty() {
        return Err(CanonicalWriteError::InvalidCandidate {
            item_key: "<part>".to_owned(),
            detail: "part has no messages".to_owned(),
        });
    }
    let mut folders = BTreeMap::<(Vec<String>, CanonicalFolderRole), Vec<MessageSpec>>::new();
    let mut item_keys = Vec::with_capacity(messages.len());
    let mut unsupported_item_keys = Vec::new();
    let mut partial = false;
    let mut omitted_properties = 0_u64;
    let mut omitted_attachments = 0_u64;
    for mail in messages {
        source.check_interrupted()?;
        let translated = translate_message(&source, mail, true)?;
        partial |= translated.partial;
        omitted_properties = omitted_properties.saturating_add(translated.omitted_properties);
        omitted_attachments = omitted_attachments.saturating_add(translated.omitted_attachments);
        folders
            .entry((mail.folder_path.clone(), mail.folder_role))
            .or_default()
            .push(translated.message);
        item_keys.extend(translated.item_keys);
        unsupported_item_keys.extend(translated.unsupported_item_keys);
    }
    let folders = folders
        .into_iter()
        .map(|((path, role), messages)| MailFolderSpec {
            path,
            role: match role {
                CanonicalFolderRole::Ordinary => MailFolderRole::Ordinary,
                CanonicalFolderRole::DeletedItems => MailFolderRole::DeletedItems,
            },
            messages,
        })
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
        partial,
        omitted_properties,
        omitted_attachments,
    })
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
}

fn translate_message(
    job: &TranslationSource<'_>,
    mail: &CanonicalMail,
    include_attachments: bool,
) -> Result<TranslatedMessage, CanonicalWriteError> {
    let mut partial = !matches!(mail.completeness, crate::ContentCompleteness::Complete);
    let mut omitted_properties = 0_u64;
    let mut omitted_attachments = 0_u64;
    let mut raw_properties = Vec::new();
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
    let mut item_keys = vec![mail.durable_item_key.clone()];
    let mut unsupported_item_keys = Vec::new();

    for property in &mail.properties {
        let Some(id) = property
            .property_id
            .and_then(|value| u16::try_from(value).ok())
        else {
            partial = true;
            omitted_properties = omitted_properties.saturating_add(1);
            continue;
        };
        let Some(property_type) = property
            .value_type
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

    let internet_codepage = internet_codepage.unwrap_or(65001);
    if internet_codepage == 65001 {
        if let Some(property) = html_property {
            if !valid_utf8_property(job, mail, property)? {
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

    let recipient_property_count = mail
        .recipients
        .iter()
        .flat_map(|recipient| &recipient.properties)
        .filter(|property| !recipient_property_is_mapped(property))
        .count();
    let recipient_property_count = saturating_len(recipient_property_count);
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
    if include_attachments {
        for attachment in &mail.attachments {
            match translate_attachment(job, mail, attachment)? {
                Some(translated) => {
                    partial |= translated.partial;
                    omitted_properties =
                        omitted_properties.saturating_add(translated.omitted_properties);
                    omitted_attachments =
                        omitted_attachments.saturating_add(translated.omitted_attachments);
                    item_keys.extend(translated.item_keys);
                    unsupported_item_keys.extend(translated.unsupported_item_keys);
                    attachments.push(translated.attachment);
                }
                None => {
                    partial = true;
                    omitted_attachments = omitted_attachments.saturating_add(1);
                }
            }
        }
    } else if !mail.attachments.is_empty() {
        partial = true;
        omitted_attachments =
            omitted_attachments.saturating_add(saturating_len(mail.attachments.len()));
        for attachment in &mail.attachments {
            if let Some(embedded) = &attachment.embedded {
                collect_message_item_keys(embedded, &mut unsupported_item_keys);
            }
        }
    }

    let message_class = mail
        .message_class
        .clone()
        .unwrap_or_else(|| "IPM.Note".to_owned());
    if !supported_message_class(&message_class) {
        return invalid(mail, format!("unsupported message class {message_class:?}"));
    }
    let subject = mail
        .subject
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "(no subject)".to_owned());
    let sender_name = mail
        .sender_name
        .clone()
        .filter(|value| !value.is_empty())
        .or_else(|| mail.sender_email.clone())
        .unwrap_or_else(|| "Unknown Sender".to_owned());
    let sender_email = mail
        .sender_email
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| sender_name.clone());
    if mail.subject.as_deref().is_none_or(str::is_empty)
        || mail.sender_name.as_deref().is_none_or(str::is_empty)
        || mail.sender_email.as_deref().is_none_or(str::is_empty)
    {
        partial = true;
    }

    let (sent_filetime, invalid_sent_filetime) = contained_filetime(mail.submit_filetime);
    let (received_filetime, invalid_received_filetime) = contained_filetime(mail.delivery_filetime);
    partial |= invalid_sent_filetime || invalid_received_filetime;
    omitted_properties = omitted_properties
        .saturating_add(u64::from(invalid_sent_filetime))
        .saturating_add(u64::from(invalid_received_filetime));
    Ok(TranslatedMessage {
        message: MessageSpec {
            message_class,
            message_flags: message_flags.unwrap_or(1),
            internet_codepage,
            subject,
            sender_name,
            sender_email,
            recipients,
            sent_filetime,
            received_filetime,
            creation_filetime: creation_filetime.unwrap_or(received_filetime),
            modification_filetime: modification_filetime.unwrap_or(received_filetime),
            body_text: None,
            body_html: None,
            body_rtf: None,
            native_body,
            rtf_in_sync,
            internet_headers: None,
            attachments,
            named_properties: Vec::new(),
            raw_properties,
            spooled_properties,
            unsupported_properties,
        },
        item_keys,
        unsupported_item_keys,
        partial,
        omitted_properties,
        omitted_attachments,
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
    attachment: AttachmentSpec,
    item_keys: Vec<String>,
    unsupported_item_keys: Vec<String>,
    partial: bool,
    omitted_properties: u64,
    omitted_attachments: u64,
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
    let mut omitted_properties = 0_u64;
    let mut mapped_property_ids = BTreeSet::new();
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
                }
            }
            id if attachment_property_is_mapped(id) => {}
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
        let translated = translate_message(job, embedded, false)?;
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
    Ok(Some(TranslatedAttachment {
        attachment: AttachmentSpec {
            filename: attachment
                .filename
                .clone()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    if attachment.embedded.is_some() {
                        format!("Embedded message {}.msg", attachment.index)
                    } else {
                        format!("Recovered attachment {}", attachment.index)
                    }
                }),
            mime_type: mime_type.or_else(|| {
                attachment
                    .embedded
                    .is_some()
                    .then(|| "message/rfc822".to_owned())
            }),
            content_id,
            content_location,
            rendering_position,
            flags,
            content,
        },
        item_keys,
        unsupported_item_keys,
        partial: (attachment.embedded.is_none() && !attachment.data_complete)
            || omitted_properties != 0
            || child_partial
            || child_omitted_properties != 0
            || child_omitted_attachments != 0,
        omitted_properties: omitted_properties.saturating_add(child_omitted_properties),
        omitted_attachments: child_omitted_attachments,
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
    class_is_or_descends_from(value, "IPM.Note") || class_descends_from(value, "REPORT.IPM.Note")
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

fn recipient_property_is_mapped(property: &CanonicalProperty) -> bool {
    property.record_set_index == 0
        && matches!(
            property.property_id,
            Some(0x0C15 | 0x3001 | 0x3002 | 0x3003 | 0x39FE)
        )
}

fn attachment_property_is_mapped(id: u32) -> bool {
    matches!(id, 0x0E20 | 0x3701 | 0x3704 | 0x3705 | 0x3707)
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

fn saturating_len(length: usize) -> u64 {
    u64::try_from(length).unwrap_or(u64::MAX)
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
    use std::io::Cursor;

    use pstforge_job::{DurableCatalogSink, SpooledBlob};
    use tempfile::tempdir;

    use super::{
        build_part_writer_input, contain_body_metadata, supported_message_class,
        valid_compressed_rtf_container, valid_utf8_stream, valid_utf16_stream,
        writer_stream_type_is_supported,
    };
    use crate::{
        CanonicalAttachment, CanonicalFolderRole, CanonicalMail, CanonicalProperty,
        ContentCompleteness, ItemKey, RecoveryProvenance,
    };
    use pstforge_pst::writer::{AttachmentContent, NativeBody};

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
            folder_role: CanonicalFolderRole::Ordinary,
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
            folder_role: CanonicalFolderRole::Ordinary,
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
        assert!(!supported_message_class("IPM.NoteCustom"));
        assert!(!supported_message_class("REPORT.IPM.NoteDR"));

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
            folder_role: CanonicalFolderRole::Ordinary,
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
        pstforge_pst::writer::create_mail_store(
            directory.path().join("report-message.pst"),
            &clean.store,
        )?;

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
        assert_eq!(nested_input.item_keys, ["normal:2:-:0", "normal:3:-:0"]);
        assert_eq!(nested_input.unsupported_item_keys, ["normal:4:-:0"]);
        assert!(nested_input.partial);
        assert_eq!(nested_input.omitted_attachments, 1);

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
        Ok(())
    }
}
