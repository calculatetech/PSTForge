use std::collections::HashSet;
use std::ptr;

use crate::bindings::{
    self, libpff_error_t, libpff_item_t, libpff_record_entry_t, libpff_record_set_t,
};
use crate::{PffError, PffFile, PffItem, check_one, free_error, native_error};

pub const STREAM_CHUNK_BYTES: usize = 64 * 1024;
const MAX_STRING_BYTES: u64 = 1024 * 1024;
const MAX_RECORD_SETS: u64 = 1_000_000;
const MAX_RECORD_ENTRIES: u64 = 1_000_000;
const MAX_MESSAGES: u64 = 100_000_000;
const MAX_EMBEDDED_DEPTH: u32 = 64;
const MAX_CATALOG_ISSUES: usize = 10_000;

const MESSAGE_CLASS: u32 = 0x001a;
const SUBJECT: u32 = 0x0037;
const SENDER_NAME: u32 = 0x0c1a;
const SENDER_EMAIL: u32 = 0x0c1f;
const RECIPIENT_COUNT: u32 = 0x0e12;
const MESSAGE_FLAGS: u32 = 0x0e07;
const MESSAGE_FLAG_HAS_ATTACHMENTS: u32 = 0x0000_0010;
const RECIPIENT_TYPE: u32 = 0x0c15;
const DISPLAY_NAME: u32 = 0x3001;
const ADDRESS_TYPE: u32 = 0x3002;
const EMAIL_ADDRESS: u32 = 0x3003;
const ATTACH_FILENAME: u32 = 0x3707;
const ATTACH_FILENAME_SHORT: u32 = 0x3704;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyOwner {
    Folder(u32),
    Message(u32),
    Recipient { message_id: u32, index: u32 },
    Attachment { message_id: u32, index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyDescriptor {
    pub owner: PropertyOwner,
    pub record_set_index: u32,
    pub entry_index: u32,
    pub entry_type: Option<u32>,
    pub value_type: Option<u32>,
    pub data_size: u64,
}

#[derive(Debug)]
pub enum CatalogEvent<'a> {
    Folder {
        id: u32,
        parent_id: Option<u32>,
        name: Option<String>,
    },
    MessageStart {
        id: u32,
        folder_id: u32,
        parent_message_id: Option<u32>,
        parent_attachment_index: Option<u32>,
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
        bytes: &'a [u8],
    },
    PropertyStart(PropertyDescriptor),
    PropertyData {
        descriptor: PropertyDescriptor,
        bytes: &'a [u8],
    },
    PropertyEnd(PropertyDescriptor),
    MessageEnd {
        id: u32,
        complete: bool,
    },
}

pub trait CatalogSink {
    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogIssue {
    pub node_id: Option<u32>,
    pub operation: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawCatalog {
    pub folders: u64,
    pub messages: u64,
    pub recipients: u64,
    pub attachments: u64,
    pub embedded_messages: u64,
    pub unsupported_messages: u64,
    pub properties: u64,
    pub property_bytes: u64,
    pub attachment_bytes: u64,
    pub issues: Vec<CatalogIssue>,
    pub issues_dropped: u64,
}

impl RawCatalog {
    fn record_issue(&mut self, issue: CatalogIssue) {
        if self.issues.len() < MAX_CATALOG_ISSUES {
            self.issues.push(issue);
        } else {
            self.issues_dropped = self.issues_dropped.saturating_add(1);
        }
    }
}

impl PffFile {
    pub fn catalog(&self, sink: &mut dyn CatalogSink) -> Result<RawCatalog, PffError> {
        let root = self.root_folder()?;
        let mut catalog = RawCatalog::default();
        let mut folders = vec![(root, None)];
        let mut visited_folders = HashSet::new();
        let mut visited_messages = HashSet::new();

        while let Some((folder, parent_id)) = folders.pop() {
            let folder_id = folder.identifier()?;
            if !visited_folders.insert(folder_id) {
                catalog.record_issue(CatalogIssue {
                    node_id: Some(folder_id),
                    operation: "traverse folder",
                    message: "duplicate or cyclic folder identifier".to_owned(),
                });
                continue;
            }
            catalog.folders = checked_increment(catalog.folders, "folder count", 1_000_000)?;
            emit(
                sink,
                "folder metadata",
                CatalogEvent::Folder {
                    id: folder_id,
                    parent_id,
                    name: folder.folder_name()?,
                },
            )?;
            stream_item_properties(
                &folder,
                PropertyOwner::Folder(folder_id),
                sink,
                &mut catalog,
            )?;

            let message_count = folder.sub_message_count()?;
            for index in 0..message_count {
                let mut messages = vec![MessageWork {
                    item: folder.sub_message(index)?,
                    folder_id,
                    parent_message_id: None,
                    parent_attachment_index: None,
                    depth: 0,
                }];
                while let Some(work) = messages.pop() {
                    process_message(
                        work,
                        &mut messages,
                        &mut visited_messages,
                        sink,
                        &mut catalog,
                    )?;
                }
            }

            let child_count = folder.sub_folder_count()?;
            for index in (0..child_count).rev() {
                folders.push((folder.sub_folder(index)?, Some(folder_id)));
            }
        }
        Ok(catalog)
    }

    fn root_folder(&self) -> Result<PffItem, PffError> {
        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is an open file and both outputs are initialized.
        let result =
            unsafe { bindings::libpff_file_get_root_folder(self.raw, &mut raw, &mut error) };
        check_one(result, error, "get root folder")?;
        PffItem::from_raw(raw, "get root folder")
    }
}

struct MessageWork {
    item: PffItem,
    folder_id: u32,
    parent_message_id: Option<u32>,
    parent_attachment_index: Option<u32>,
    depth: u32,
}

fn process_message(
    work: MessageWork,
    pending: &mut Vec<MessageWork>,
    visited: &mut HashSet<u32>,
    sink: &mut dyn CatalogSink,
    catalog: &mut RawCatalog,
) -> Result<(), PffError> {
    let message_id = work.item.identifier()?;
    if !visited.insert(message_id) {
        catalog.record_issue(CatalogIssue {
            node_id: Some(message_id),
            operation: "traverse message",
            message: "duplicate message identifier".to_owned(),
        });
        return Ok(());
    }
    catalog.messages = checked_increment(catalog.messages, "message count", MAX_MESSAGES)?;
    if work.parent_message_id.is_some() {
        catalog.embedded_messages =
            catalog
                .embedded_messages
                .checked_add(1)
                .ok_or(PffError::LimitExceeded {
                    field: "embedded message count",
                    value: u64::MAX,
                    limit: u64::MAX - 1,
                })?;
    }
    let message_class = work.item.message_string(MESSAGE_CLASS)?;
    let recipient_count_hint = work.item.optional_u32(RECIPIENT_COUNT)?;
    let has_attachments = work
        .item
        .optional_u32(MESSAGE_FLAGS)?
        .map(|flags| flags & MESSAGE_FLAG_HAS_ATTACHMENTS != 0);
    let supported = message_class
        .as_deref()
        .is_some_and(|value| value.starts_with("IPM.Note") || value.starts_with("REPORT.IPM.Note"));
    if !supported {
        catalog.unsupported_messages =
            catalog
                .unsupported_messages
                .checked_add(1)
                .ok_or(PffError::LimitExceeded {
                    field: "unsupported message count",
                    value: u64::MAX,
                    limit: u64::MAX - 1,
                })?;
    }
    emit(
        sink,
        "message metadata",
        CatalogEvent::MessageStart {
            id: message_id,
            folder_id: work.folder_id,
            parent_message_id: work.parent_message_id,
            parent_attachment_index: work.parent_attachment_index,
            item_type: work.item.item_type()?,
            message_class,
            subject: work.item.message_string(SUBJECT)?,
            sender_name: work.item.message_string(SENDER_NAME)?,
            sender_email: work.item.message_string(SENDER_EMAIL)?,
            submit_filetime: work.item.optional_filetime(
                "get client submit time",
                bindings::libpff_message_get_client_submit_time,
            )?,
            delivery_filetime: work.item.optional_filetime(
                "get delivery time",
                bindings::libpff_message_get_delivery_time,
            )?,
            supported,
        },
    )?;
    let issue_state = (catalog.issues.len(), catalog.issues_dropped);
    if let Err(error) =
        stream_recipients(&work.item, message_id, recipient_count_hint, sink, catalog)
    {
        catalog.record_issue(catalog_issue(Some(message_id), "stream recipients", error));
    }
    if let Err(error) =
        stream_attachments(&work, message_id, has_attachments, pending, sink, catalog)
    {
        catalog.record_issue(catalog_issue(Some(message_id), "stream attachments", error));
    }
    if let Err(error) = stream_item_properties(
        &work.item,
        PropertyOwner::Message(message_id),
        sink,
        catalog,
    ) {
        catalog.record_issue(catalog_issue(
            Some(message_id),
            "stream message properties",
            error,
        ));
    }
    emit(
        sink,
        "message end",
        CatalogEvent::MessageEnd {
            id: message_id,
            complete: (catalog.issues.len(), catalog.issues_dropped) == issue_state,
        },
    )
}

fn stream_recipients(
    message: &PffItem,
    message_id: u32,
    count_hint: Option<u32>,
    sink: &mut dyn CatalogSink,
    catalog: &mut RawCatalog,
) -> Result<(), PffError> {
    if count_hint == Some(0) {
        return Ok(());
    }
    let Some(recipients) =
        message.optional_item("get recipients", bindings::libpff_message_get_recipients)?
    else {
        return Ok(());
    };
    let count = recipients.record_set_count()?;
    for index in 0..count {
        let record_set = recipients.record_set(index)?;
        let index_u32 = checked_index(index, "recipient index")?;
        emit(
            sink,
            "recipient metadata",
            CatalogEvent::Recipient {
                message_id,
                index: index_u32,
                recipient_type: record_set.optional_u32(RECIPIENT_TYPE)?,
                display_name: record_set.optional_string(DISPLAY_NAME)?,
                email_address: record_set.optional_string(EMAIL_ADDRESS)?,
                address_type: record_set.optional_string(ADDRESS_TYPE)?,
            },
        )?;
        catalog.recipients = catalog
            .recipients
            .checked_add(1)
            .ok_or(PffError::LimitExceeded {
                field: "recipient count",
                value: u64::MAX,
                limit: u64::MAX - 1,
            })?;
        stream_record_set(
            &record_set,
            PropertyOwner::Recipient {
                message_id,
                index: index_u32,
            },
            checked_index(index, "record set index")?,
            sink,
            catalog,
        )?;
    }
    Ok(())
}

fn stream_attachments(
    work: &MessageWork,
    message_id: u32,
    has_attachments: Option<bool>,
    pending: &mut Vec<MessageWork>,
    sink: &mut dyn CatalogSink,
    catalog: &mut RawCatalog,
) -> Result<(), PffError> {
    if has_attachments == Some(false) {
        return Ok(());
    }
    let count = work.item.attachment_count()?;
    for index in 0..count {
        let attachment = work.item.attachment(index)?;
        let index_u32 = checked_index(index, "attachment index")?;
        let attachment_type = attachment.attachment_type()?;
        let data_size = attachment.attachment_size()?;
        let filename = attachment
            .first_record_set()?
            .map(|set| {
                set.optional_string(ATTACH_FILENAME).and_then(|value| {
                    if value.is_some() {
                        Ok(value)
                    } else {
                        set.optional_string(ATTACH_FILENAME_SHORT)
                    }
                })
            })
            .transpose()?
            .flatten();
        emit(
            sink,
            "attachment metadata",
            CatalogEvent::AttachmentStart {
                message_id,
                index: index_u32,
                attachment_type,
                data_size,
                filename,
            },
        )?;
        catalog.attachments =
            catalog
                .attachments
                .checked_add(1)
                .ok_or(PffError::LimitExceeded {
                    field: "attachment count",
                    value: u64::MAX,
                    limit: u64::MAX - 1,
                })?;
        stream_item_properties(
            &attachment,
            PropertyOwner::Attachment {
                message_id,
                index: index_u32,
            },
            sink,
            catalog,
        )?;
        if let Some(expected) = data_size {
            let actual = attachment.stream_attachment(message_id, index_u32, sink)?;
            if actual != expected {
                return Err(PffError::StreamSizeMismatch {
                    field: "attachment data",
                    expected,
                    actual,
                });
            }
            catalog.attachment_bytes =
                catalog
                    .attachment_bytes
                    .checked_add(actual)
                    .ok_or(PffError::LimitExceeded {
                        field: "attachment byte count",
                        value: u64::MAX,
                        limit: u64::MAX - 1,
                    })?;
        }
        if attachment_type == Some(i32::from(b'i')) {
            if let Some(embedded) = attachment
                .optional_item("get embedded item", bindings::libpff_attachment_get_item)?
            {
                let depth = work.depth.checked_add(1).ok_or(PffError::LimitExceeded {
                    field: "embedded message depth",
                    value: u64::MAX,
                    limit: u64::from(MAX_EMBEDDED_DEPTH),
                })?;
                if depth > MAX_EMBEDDED_DEPTH {
                    return Err(PffError::LimitExceeded {
                        field: "embedded message depth",
                        value: u64::from(depth),
                        limit: u64::from(MAX_EMBEDDED_DEPTH),
                    });
                }
                pending.push(MessageWork {
                    item: embedded,
                    folder_id: work.folder_id,
                    parent_message_id: Some(message_id),
                    parent_attachment_index: Some(index_u32),
                    depth,
                });
            }
        }
    }
    Ok(())
}

fn stream_item_properties(
    item: &PffItem,
    owner: PropertyOwner,
    sink: &mut dyn CatalogSink,
    catalog: &mut RawCatalog,
) -> Result<(), PffError> {
    let count = item.record_set_count()?;
    for index in 0..count {
        let set = item.record_set(index)?;
        stream_record_set(
            &set,
            owner,
            checked_index(index, "record set index")?,
            sink,
            catalog,
        )?;
    }
    Ok(())
}

fn stream_record_set(
    set: &PffRecordSet,
    owner: PropertyOwner,
    record_set_index: u32,
    sink: &mut dyn CatalogSink,
    catalog: &mut RawCatalog,
) -> Result<(), PffError> {
    let count = set.entry_count()?;
    for index in 0..count {
        let entry = set.entry(index)?;
        let descriptor = PropertyDescriptor {
            owner,
            record_set_index,
            entry_index: checked_index(index, "record entry index")?,
            entry_type: entry.optional_type(
                "get record entry type",
                bindings::libpff_record_entry_get_entry_type,
            )?,
            value_type: entry.optional_type(
                "get record value type",
                bindings::libpff_record_entry_get_value_type,
            )?,
            data_size: entry.data_size()?,
        };
        emit(
            sink,
            "property start",
            CatalogEvent::PropertyStart(descriptor),
        )?;
        let actual = entry.stream(descriptor, sink)?;
        if actual != descriptor.data_size {
            return Err(PffError::StreamSizeMismatch {
                field: "property data",
                expected: descriptor.data_size,
                actual,
            });
        }
        emit(sink, "property end", CatalogEvent::PropertyEnd(descriptor))?;
        catalog.properties = catalog
            .properties
            .checked_add(1)
            .ok_or(PffError::LimitExceeded {
                field: "property count",
                value: u64::MAX,
                limit: u64::MAX - 1,
            })?;
        catalog.property_bytes =
            catalog
                .property_bytes
                .checked_add(actual)
                .ok_or(PffError::LimitExceeded {
                    field: "property byte count",
                    value: u64::MAX,
                    limit: u64::MAX - 1,
                })?;
    }
    Ok(())
}

struct PffRecordSet {
    raw: *mut libpff_record_set_t,
}

impl PffRecordSet {
    fn entry_count(&self) -> Result<u64, PffError> {
        native_count(
            self.raw,
            "get number of record entries",
            bindings::libpff_record_set_get_number_of_entries,
            MAX_RECORD_ENTRIES,
        )
    }

    fn entry(&self, index: u64) -> Result<PffRecordEntry, PffError> {
        let index = native_index(index, "record entry index")?;
        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result = unsafe {
            bindings::libpff_record_set_get_entry_by_index(self.raw, index, &mut raw, &mut error)
        };
        check_one(result, error, "get record entry")?;
        PffRecordEntry::from_raw(raw, "get record entry")
    }

    fn optional_entry(&self, entry_type: u32) -> Result<Option<PffRecordEntry>, PffError> {
        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized. Value type zero and flag one mean any type.
        let result = unsafe {
            bindings::libpff_record_set_get_entry_by_type(
                self.raw, entry_type, 0, &mut raw, 1, &mut error,
            )
        };
        match result {
            1 => {
                free_error(error);
                PffRecordEntry::from_raw(raw, "get record entry by type").map(Some)
            }
            0 => {
                free_error(error);
                Ok(None)
            }
            _ => Err(native_error(error, "get record entry by type")),
        }
    }

    fn optional_string(&self, entry_type: u32) -> Result<Option<String>, PffError> {
        self.optional_entry(entry_type)?
            .map(|entry| entry.string())
            .transpose()
    }

    fn optional_u32(&self, entry_type: u32) -> Result<Option<u32>, PffError> {
        self.optional_entry(entry_type)?
            .map(|entry| entry.u32())
            .transpose()
    }
}

impl Drop for PffRecordSet {
    fn drop(&mut self) {
        if self.raw.is_null() {
            return;
        }
        let mut error = ptr::null_mut();
        // SAFETY: this wrapper owns the record set and frees it once.
        unsafe {
            bindings::libpff_record_set_free(&mut self.raw, &mut error);
        }
        free_error(error);
    }
}

struct PffRecordEntry {
    raw: *mut libpff_record_entry_t,
}

impl PffRecordEntry {
    fn from_raw(
        raw: *mut libpff_record_entry_t,
        operation: &'static str,
    ) -> Result<Self, PffError> {
        if raw.is_null() {
            Err(PffError::NullPointer { operation })
        } else {
            Ok(Self { raw })
        }
    }

    fn optional_type(
        &self,
        operation: &'static str,
        function: unsafe extern "C" fn(
            *mut libpff_record_entry_t,
            *mut u32,
            *mut *mut libpff_error_t,
        ) -> i32,
    ) -> Result<Option<u32>, PffError> {
        let mut value = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result = unsafe { function(self.raw, &mut value, &mut error) };
        match result {
            1 => {
                free_error(error);
                Ok(Some(value))
            }
            0 => {
                free_error(error);
                Ok(None)
            }
            _ => Err(native_error(error, operation)),
        }
    }

    fn data_size(&self) -> Result<u64, PffError> {
        let mut size = 0_usize;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result =
            unsafe { bindings::libpff_record_entry_get_data_size(self.raw, &mut size, &mut error) };
        check_one(result, error, "get record data size")?;
        u64::try_from(size).map_err(|_| PffError::LimitExceeded {
            field: "record data size",
            value: u64::MAX,
            limit: u64::MAX - 1,
        })
    }

    fn string(&self) -> Result<String, PffError> {
        read_optional_string(
            self.raw,
            "get record string size",
            "get record string",
            bindings::libpff_record_entry_get_data_as_utf8_string_size,
            bindings::libpff_record_entry_get_data_as_utf8_string,
        )?
        .ok_or(PffError::Native {
            operation: "get record string",
            detail: "value is absent".to_owned(),
        })
    }

    fn u32(&self) -> Result<u32, PffError> {
        let mut value = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result = unsafe {
            bindings::libpff_record_entry_get_data_as_32bit_integer(
                self.raw, &mut value, &mut error,
            )
        };
        check_one(result, error, "get record integer")?;
        Ok(value)
    }

    fn stream(
        &self,
        descriptor: PropertyDescriptor,
        sink: &mut dyn CatalogSink,
    ) -> Result<u64, PffError> {
        let mut total = 0_u64;
        let mut buffer = vec![0_u8; STREAM_CHUNK_BYTES];
        loop {
            let mut error = ptr::null_mut();
            // SAFETY: self.raw is valid and buffer is writable for its declared size.
            let read = unsafe {
                bindings::libpff_record_entry_read_buffer(
                    self.raw,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut error,
                )
            };
            if read < 0 {
                return Err(native_error(error, "read record data"));
            }
            free_error(error);
            if read == 0 {
                break;
            }
            let read = usize::try_from(read).map_err(|_| PffError::InvalidValue {
                field: "record read size",
                value: i64::MAX,
            })?;
            if read > buffer.len() {
                return Err(PffError::InvalidValue {
                    field: "record read size",
                    value: i64::try_from(read).unwrap_or(i64::MAX),
                });
            }
            total = total
                .checked_add(u64::try_from(read).map_err(|_| PffError::InvalidValue {
                    field: "record read size",
                    value: i64::MAX,
                })?)
                .ok_or(PffError::LimitExceeded {
                    field: "record streamed size",
                    value: u64::MAX,
                    limit: u64::MAX - 1,
                })?;
            if total > descriptor.data_size {
                return Err(PffError::StreamSizeMismatch {
                    field: "property data",
                    expected: descriptor.data_size,
                    actual: total,
                });
            }
            emit(
                sink,
                "property data",
                CatalogEvent::PropertyData {
                    descriptor,
                    bytes: &buffer[..read],
                },
            )?;
        }
        Ok(total)
    }
}

impl Drop for PffRecordEntry {
    fn drop(&mut self) {
        if self.raw.is_null() {
            return;
        }
        let mut error = ptr::null_mut();
        // SAFETY: this wrapper owns the record entry and frees it once.
        unsafe {
            bindings::libpff_record_entry_free(&mut self.raw, &mut error);
        }
        free_error(error);
    }
}

impl PffItem {
    fn from_raw(raw: *mut libpff_item_t, operation: &'static str) -> Result<Self, PffError> {
        if raw.is_null() {
            Err(PffError::NullPointer { operation })
        } else {
            Ok(Self { raw })
        }
    }

    fn item_type(&self) -> Result<Option<u8>, PffError> {
        let mut value = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result = unsafe { bindings::libpff_item_get_type(self.raw, &mut value, &mut error) };
        match result {
            1 => {
                free_error(error);
                Ok(Some(value))
            }
            0 => {
                free_error(error);
                Ok(None)
            }
            _ => Err(native_error(error, "get item type")),
        }
    }

    fn optional_u32(&self, entry_type: u32) -> Result<Option<u32>, PffError> {
        let mut value = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and record set zero is the message property set.
        let result = unsafe {
            bindings::libpff_item_get_entry_value_32bit(
                self.raw, 0, entry_type, &mut value, 0, &mut error,
            )
        };
        match result {
            1 => {
                free_error(error);
                Ok(Some(value))
            }
            0 => {
                free_error(error);
                Ok(None)
            }
            _ => Err(native_error(error, "get item integer")),
        }
    }

    fn folder_name(&self) -> Result<Option<String>, PffError> {
        read_optional_string(
            self.raw,
            "get folder name size",
            "get folder name",
            bindings::libpff_folder_get_utf8_name_size,
            bindings::libpff_folder_get_utf8_name,
        )
    }

    fn message_string(&self, entry_type: u32) -> Result<Option<String>, PffError> {
        read_optional_message_string(self.raw, entry_type)
    }

    fn sub_message(&self, index: u64) -> Result<Self, PffError> {
        let index = native_index(index, "message index")?;
        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result = unsafe {
            bindings::libpff_folder_get_sub_message(self.raw, index, &mut raw, &mut error)
        };
        check_one(result, error, "get submessage")?;
        Self::from_raw(raw, "get submessage")
    }

    fn record_set_count(&self) -> Result<u64, PffError> {
        native_count(
            self.raw,
            "get number of record sets",
            bindings::libpff_item_get_number_of_record_sets,
            MAX_RECORD_SETS,
        )
    }

    fn record_set(&self, index: u64) -> Result<PffRecordSet, PffError> {
        let index = native_index(index, "record set index")?;
        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result = unsafe {
            bindings::libpff_item_get_record_set_by_index(self.raw, index, &mut raw, &mut error)
        };
        check_one(result, error, "get record set")?;
        if raw.is_null() {
            Err(PffError::NullPointer {
                operation: "get record set",
            })
        } else {
            Ok(PffRecordSet { raw })
        }
    }

    fn first_record_set(&self) -> Result<Option<PffRecordSet>, PffError> {
        if self.record_set_count()? == 0 {
            Ok(None)
        } else {
            self.record_set(0).map(Some)
        }
    }

    fn optional_filetime(
        &self,
        operation: &'static str,
        function: unsafe extern "C" fn(
            *mut libpff_item_t,
            *mut u64,
            *mut *mut libpff_error_t,
        ) -> i32,
    ) -> Result<Option<u64>, PffError> {
        let mut value = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result = unsafe { function(self.raw, &mut value, &mut error) };
        match result {
            1 => {
                free_error(error);
                Ok(Some(value))
            }
            0 => {
                free_error(error);
                Ok(None)
            }
            _ => Err(native_error(error, operation)),
        }
    }

    fn attachment_count(&self) -> Result<u64, PffError> {
        native_count(
            self.raw,
            "get number of attachments",
            bindings::libpff_message_get_number_of_attachments,
            MAX_RECORD_ENTRIES,
        )
    }

    fn attachment(&self, index: u64) -> Result<Self, PffError> {
        let index = native_index(index, "attachment index")?;
        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result = unsafe {
            bindings::libpff_message_get_attachment(self.raw, index, &mut raw, &mut error)
        };
        check_one(result, error, "get attachment")?;
        Self::from_raw(raw, "get attachment")
    }

    fn optional_item(
        &self,
        operation: &'static str,
        function: unsafe extern "C" fn(
            *mut libpff_item_t,
            *mut *mut libpff_item_t,
            *mut *mut libpff_error_t,
        ) -> i32,
    ) -> Result<Option<Self>, PffError> {
        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result = unsafe { function(self.raw, &mut raw, &mut error) };
        match result {
            1 => {
                free_error(error);
                Self::from_raw(raw, operation).map(Some)
            }
            0 => {
                free_error(error);
                Ok(None)
            }
            _ => Err(native_error(error, operation)),
        }
    }

    fn attachment_type(&self) -> Result<Option<i32>, PffError> {
        let mut value = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result =
            unsafe { bindings::libpff_attachment_get_type(self.raw, &mut value, &mut error) };
        match result {
            1 => {
                free_error(error);
                Ok(Some(value))
            }
            0 => {
                free_error(error);
                Ok(None)
            }
            _ => Err(native_error(error, "get attachment type")),
        }
    }

    fn attachment_size(&self) -> Result<Option<u64>, PffError> {
        let mut value = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result =
            unsafe { bindings::libpff_attachment_get_data_size(self.raw, &mut value, &mut error) };
        match result {
            1 => {
                free_error(error);
                Ok(Some(value))
            }
            0 => {
                free_error(error);
                Ok(None)
            }
            _ => Err(native_error(error, "get attachment size")),
        }
    }

    fn stream_attachment(
        &self,
        message_id: u32,
        index: u32,
        sink: &mut dyn CatalogSink,
    ) -> Result<u64, PffError> {
        let mut total = 0_u64;
        let mut buffer = vec![0_u8; STREAM_CHUNK_BYTES];
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid; offset zero with SEEK_SET resets the owned data stream.
        let offset =
            unsafe { bindings::libpff_attachment_data_seek_offset(self.raw, 0, 0, &mut error) };
        if offset != 0 {
            return Err(native_error(error, "seek attachment data"));
        }
        free_error(error);
        loop {
            error = ptr::null_mut();
            // SAFETY: self.raw is valid and buffer is writable for its declared size.
            let read = unsafe {
                bindings::libpff_attachment_data_read_buffer(
                    self.raw,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut error,
                )
            };
            if read < 0 {
                return Err(native_error(error, "read attachment data"));
            }
            free_error(error);
            if read == 0 {
                break;
            }
            let read = usize::try_from(read).map_err(|_| PffError::InvalidValue {
                field: "attachment read size",
                value: i64::MAX,
            })?;
            if read > buffer.len() {
                return Err(PffError::InvalidValue {
                    field: "attachment read size",
                    value: i64::try_from(read).unwrap_or(i64::MAX),
                });
            }
            total = total
                .checked_add(u64::try_from(read).map_err(|_| PffError::InvalidValue {
                    field: "attachment read size",
                    value: i64::MAX,
                })?)
                .ok_or(PffError::LimitExceeded {
                    field: "attachment streamed size",
                    value: u64::MAX,
                    limit: u64::MAX - 1,
                })?;
            emit(
                sink,
                "attachment data",
                CatalogEvent::AttachmentData {
                    message_id,
                    index,
                    bytes: &buffer[..read],
                },
            )?;
        }
        Ok(total)
    }
}

fn read_optional_message_string(
    item: *mut libpff_item_t,
    entry_type: u32,
) -> Result<Option<String>, PffError> {
    let mut size = 0_usize;
    let mut error = ptr::null_mut();
    // SAFETY: item is valid and outputs are initialized.
    let result = unsafe {
        bindings::libpff_message_get_entry_value_utf8_string_size(
            item, entry_type, &mut size, &mut error,
        )
    };
    match result {
        0 => {
            free_error(error);
            return Ok(None);
        }
        1 => free_error(error),
        _ => return Err(native_error(error, "get message string size")),
    }
    if size == 0 {
        return Ok(None);
    }
    validate_string_size(size)?;
    let mut bytes = vec![0_u8; size];
    error = ptr::null_mut();
    // SAFETY: item is valid and bytes is writable for size bytes.
    let result = unsafe {
        bindings::libpff_message_get_entry_value_utf8_string(
            item,
            entry_type,
            bytes.as_mut_ptr(),
            bytes.len(),
            &mut error,
        )
    };
    check_one(result, error, "get message string")?;
    Ok(Some(decode_native_string(bytes)))
}

fn read_optional_string<T>(
    raw: *mut T,
    size_operation: &'static str,
    read_operation: &'static str,
    size_fn: unsafe extern "C" fn(*mut T, *mut usize, *mut *mut libpff_error_t) -> i32,
    read_fn: unsafe extern "C" fn(*mut T, *mut u8, usize, *mut *mut libpff_error_t) -> i32,
) -> Result<Option<String>, PffError> {
    let mut size = 0_usize;
    let mut error = ptr::null_mut();
    // SAFETY: raw is a valid native object and outputs are initialized.
    let result = unsafe { size_fn(raw, &mut size, &mut error) };
    match result {
        0 => {
            free_error(error);
            return Ok(None);
        }
        1 => free_error(error),
        _ => return Err(native_error(error, size_operation)),
    }
    if size == 0 {
        return Ok(None);
    }
    validate_string_size(size)?;
    let mut bytes = vec![0_u8; size];
    error = ptr::null_mut();
    // SAFETY: raw is valid and bytes is writable for size bytes.
    let result = unsafe { read_fn(raw, bytes.as_mut_ptr(), bytes.len(), &mut error) };
    check_one(result, error, read_operation)?;
    Ok(Some(decode_native_string(bytes)))
}

fn decode_native_string(mut bytes: Vec<u8>) -> String {
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn validate_string_size(size: usize) -> Result<(), PffError> {
    let size_u64 = u64::try_from(size).map_err(|_| PffError::LimitExceeded {
        field: "string size",
        value: u64::MAX,
        limit: MAX_STRING_BYTES,
    })?;
    if size_u64 > MAX_STRING_BYTES {
        return Err(PffError::LimitExceeded {
            field: "string size",
            value: size_u64,
            limit: MAX_STRING_BYTES,
        });
    }
    Ok(())
}

fn native_count<T>(
    raw: *mut T,
    operation: &'static str,
    function: unsafe extern "C" fn(*mut T, *mut i32, *mut *mut libpff_error_t) -> i32,
    limit: u64,
) -> Result<u64, PffError> {
    let mut count = 0;
    let mut error = ptr::null_mut();
    // SAFETY: raw is valid and outputs are initialized.
    let result = unsafe { function(raw, &mut count, &mut error) };
    check_one(result, error, operation)?;
    let count = u64::try_from(count).map_err(|_| PffError::InvalidValue {
        field: "native count",
        value: i64::from(count),
    })?;
    if count > limit {
        return Err(PffError::LimitExceeded {
            field: "native count",
            value: count,
            limit,
        });
    }
    Ok(count)
}

fn native_index(index: u64, field: &'static str) -> Result<i32, PffError> {
    i32::try_from(index).map_err(|_| PffError::LimitExceeded {
        field,
        value: index,
        limit: i32::MAX as u64,
    })
}

fn checked_index(index: u64, field: &'static str) -> Result<u32, PffError> {
    u32::try_from(index).map_err(|_| PffError::LimitExceeded {
        field,
        value: index,
        limit: u64::from(u32::MAX),
    })
}

fn checked_increment(value: u64, field: &'static str, limit: u64) -> Result<u64, PffError> {
    let value = value.checked_add(1).ok_or(PffError::LimitExceeded {
        field,
        value: u64::MAX,
        limit,
    })?;
    if value > limit {
        return Err(PffError::LimitExceeded {
            field,
            value,
            limit,
        });
    }
    Ok(value)
}

fn emit(
    sink: &mut dyn CatalogSink,
    operation: &'static str,
    event: CatalogEvent<'_>,
) -> Result<(), PffError> {
    sink.event(event)
        .map_err(|detail| PffError::Sink { operation, detail })
}

fn catalog_issue(node_id: Option<u32>, operation: &'static str, error: PffError) -> CatalogIssue {
    CatalogIssue {
        node_id,
        operation,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CatalogEvent, CatalogIssue, CatalogSink, MAX_CATALOG_ISSUES, PropertyDescriptor,
        PropertyOwner, RawCatalog, STREAM_CHUNK_BYTES, checked_increment, decode_native_string,
        validate_string_size,
    };

    struct BoundedSink {
        peak: usize,
    }

    impl CatalogSink for BoundedSink {
        fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
            let size = match event {
                CatalogEvent::AttachmentData { bytes, .. }
                | CatalogEvent::PropertyData { bytes, .. } => bytes.len(),
                _ => 0,
            };
            self.peak = self.peak.max(size);
            Ok(())
        }
    }

    #[test]
    fn native_strings_drop_only_the_trailing_nul() {
        assert_eq!(decode_native_string(b"hello\0".to_vec()), "hello");
        assert_eq!(decode_native_string(b"hello".to_vec()), "hello");
    }

    #[test]
    fn length_and_counter_limits_are_enforced() {
        assert!(validate_string_size(1024).is_ok());
        assert!(validate_string_size(1024 * 1024 + 1).is_err());
        assert!(checked_increment(2, "test", 3).is_ok());
        assert!(checked_increment(3, "test", 3).is_err());
    }

    #[test]
    fn sink_observes_bounded_chunks() {
        let mut sink = BoundedSink { peak: 0 };
        let bytes = vec![0_u8; STREAM_CHUNK_BYTES];
        let descriptor = PropertyDescriptor {
            owner: PropertyOwner::Folder(1),
            record_set_index: 0,
            entry_index: 0,
            entry_type: None,
            value_type: None,
            data_size: bytes.len() as u64,
        };
        assert!(
            sink.event(CatalogEvent::PropertyData {
                descriptor,
                bytes: &bytes
            })
            .is_ok()
        );
        assert_eq!(sink.peak, STREAM_CHUNK_BYTES);
    }

    #[test]
    fn retained_issue_count_is_bounded() {
        let mut catalog = RawCatalog::default();
        for _ in 0..=MAX_CATALOG_ISSUES {
            catalog.record_issue(CatalogIssue {
                node_id: None,
                operation: "test",
                message: "test".to_owned(),
            });
        }
        assert_eq!(catalog.issues.len(), MAX_CATALOG_ISSUES);
        assert_eq!(catalog.issues_dropped, 1);
    }
}
