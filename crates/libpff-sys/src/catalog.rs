use std::collections::HashSet;
use std::ptr;

use serde::{Deserialize, Serialize};

use crate::bindings::{
    self, libpff_error_t, libpff_item_t, libpff_record_entry_t, libpff_record_set_t,
};
use crate::{PffError, PffFile, PffItem, check_one, free_error, native_error};

pub const STREAM_CHUNK_BYTES: usize = 64 * 1024;
const MAX_STRING_BYTES: u64 = 1024 * 1024;
const MAX_RECORD_SETS: u64 = 1_000_000;
const MAX_RECORD_ENTRIES: u64 = 1_000_000;
const MAX_MESSAGES: u64 = 100_000_000;
const MAX_EMBEDDED_DEPTH: u32 = 256;
const MAX_FOLDER_DEPTH: usize = 64;
const MAX_CATALOG_ISSUES: usize = 10_000;
const RECOVERY_FLAG_IGNORE_ALLOCATION_DATA: u8 = 0x01;
const RECOVERY_FLAG_SCAN_FOR_FRAGMENTS: u8 = 0x02;

struct RecoveryCollectionFunctions {
    count: unsafe extern "C" fn(
        *mut bindings::libpff_file_t,
        *mut i32,
        *mut *mut libpff_error_t,
    ) -> i32,
    item: unsafe extern "C" fn(
        *mut bindings::libpff_file_t,
        i32,
        *mut *mut libpff_item_t,
        *mut *mut libpff_error_t,
    ) -> i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogProvenance {
    Normal,
    Recovered,
    Orphan,
    Fragment,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMode {
    #[default]
    Balanced,
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FolderAddress {
    head: [u32; 32],
    tail: [u32; 32],
    depth: u8,
}

impl FolderAddress {
    pub fn root() -> Self {
        Self {
            head: [0; 32],
            tail: [0; 32],
            depth: 0,
        }
    }

    pub fn child(self, index: u32) -> Option<Self> {
        let depth = usize::from(self.depth);
        if depth >= MAX_FOLDER_DEPTH {
            return None;
        }
        let mut child = self;
        if depth < 32 {
            child.head[depth] = index;
        } else {
            child.tail[depth - 32] = index;
        }
        child.depth = child.depth.checked_add(1)?;
        Some(child)
    }

    pub fn parent(self) -> Option<Self> {
        let depth = self.depth.checked_sub(1)?;
        let mut parent = self;
        let index = usize::from(depth);
        if index < 32 {
            parent.head[index] = 0;
        } else {
            parent.tail[index - 32] = 0;
        }
        parent.depth = depth;
        Some(parent)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecoveryUnit {
    Folder {
        address: FolderAddress,
    },
    ChildFolder {
        address: FolderAddress,
    },
    Normal {
        folder: FolderAddress,
        folder_id: u32,
        message_index: u64,
    },
    Associated {
        folder: FolderAddress,
        folder_id: u32,
        message_index: u64,
    },
    Recovered {
        index: u64,
    },
    Orphan {
        index: u64,
    },
    Fragment {
        index: u64,
    },
}

const MESSAGE_CLASS: u32 = 0x001a;
const SUBJECT: u32 = 0x0037;
const SENDER_NAME: u32 = 0x0c1a;
const SENDER_EMAIL: u32 = 0x0c1f;
const RECIPIENT_COUNT: u32 = 0x0e12;
const RECIPIENT_TYPE: u32 = 0x0c15;
const DISPLAY_NAME: u32 = 0x3001;
const ADDRESS_TYPE: u32 = 0x3002;
const EMAIL_ADDRESS: u32 = 0x3003;
const ATTACH_FILENAME: u32 = 0x3707;
const ATTACH_FILENAME_SHORT: u32 = 0x3704;
const ATTACH_METHOD: u32 = 0x3705;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyOwner {
    Folder(u32),
    Message(u32),
    Recipient { message_id: u32, index: u32 },
    Attachment { message_id: u32, index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyDescriptor {
    pub owner: PropertyOwner,
    pub record_set_index: u32,
    pub entry_index: u32,
    pub entry_type: Option<u32>,
    pub value_type: Option<u32>,
    pub data_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum NamedPropertyName {
    Numeric(u32),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedPropertyIdentity {
    pub guid: [u8; 16],
    pub name: NamedPropertyName,
}

#[derive(Debug)]
pub enum CatalogEvent<'a> {
    UnitStart(RecoveryUnit),
    UnitEnd(RecoveryUnit),
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
        bytes: &'a [u8],
    },
    AttachmentEnd {
        message_id: u32,
        index: u32,
    },
    AttachmentAbort {
        message_id: u32,
        index: u32,
    },
    PropertyStart(PropertyDescriptor),
    NamedProperty {
        descriptor: PropertyDescriptor,
        identity: NamedPropertyIdentity,
    },
    PropertyData {
        descriptor: PropertyDescriptor,
        bytes: &'a [u8],
    },
    PropertyEnd(PropertyDescriptor),
    PropertyAbort {
        descriptor: PropertyDescriptor,
        reason: String,
    },
    MessageEnd {
        id: u32,
        complete: bool,
    },
}

pub trait CatalogSink {
    fn property_payload(&self, _descriptor: PropertyDescriptor) -> PayloadRequest {
        PayloadRequest::Full
    }

    fn attachment_payload(
        &self,
        _message_id: u32,
        _index: u32,
        _declared_size: Option<u64>,
    ) -> PayloadRequest {
        PayloadRequest::Full
    }

    fn traversal_order(&self) -> TraversalOrder {
        TraversalOrder::Source
    }

    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadRequest {
    Full,
    Prefix(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalOrder {
    Source,
    EmbeddedFirst,
    Writer,
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
    pub recovered_messages: u64,
    pub orphan_messages: u64,
    pub fragment_messages: u64,
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
        self.catalog_reachable(sink, &HashSet::new())
            .map(|(catalog, _)| catalog)
    }

    pub fn recovery_catalog(&mut self, sink: &mut dyn CatalogSink) -> Result<RawCatalog, PffError> {
        self.recovery_catalog_skipping(sink, &HashSet::new(), RecoveryMode::Balanced)
    }

    pub fn recovery_catalog_skipping(
        &mut self,
        sink: &mut dyn CatalogSink,
        skipped: &HashSet<RecoveryUnit>,
        mode: RecoveryMode,
    ) -> Result<RawCatalog, PffError> {
        let (mut catalog, mut visited_messages) = match self.catalog_reachable(sink, skipped) {
            Ok(reachable) => reachable,
            Err(
                error @ (PffError::Native { .. }
                | PffError::MissingRootFolder
                | PffError::NullPointer { .. }),
            ) => {
                let mut catalog = RawCatalog::default();
                catalog.record_issue(catalog_issue(None, "traverse normal folder tree", error));
                (catalog, HashSet::new())
            }
            Err(error) => return Err(error),
        };
        let mut error = ptr::null_mut();
        let recovery_flags = recovery_flags(mode);
        // SAFETY: self.raw is an open file and recovery_flags contains only libpff-defined flags.
        let result =
            unsafe { bindings::libpff_file_recover_items(self.raw, recovery_flags, &mut error) };
        check_one(result, error, "recover items")?;
        let recovered_provenance = recovered_provenance(mode);
        self.stream_recovery_collection(
            recovered_provenance,
            RecoveryCollectionFunctions {
                count: bindings::libpff_file_get_number_of_recovered_items,
                item: bindings::libpff_file_get_recovered_item_by_index,
            },
            &mut visited_messages,
            sink,
            &mut catalog,
            skipped,
        )?;
        self.stream_recovery_collection(
            CatalogProvenance::Orphan,
            RecoveryCollectionFunctions {
                count: bindings::libpff_file_get_number_of_orphan_items,
                item: bindings::libpff_file_get_orphan_item_by_index,
            },
            &mut visited_messages,
            sink,
            &mut catalog,
            skipped,
        )?;
        Ok(catalog)
    }

    fn catalog_reachable(
        &self,
        sink: &mut dyn CatalogSink,
        skipped: &HashSet<RecoveryUnit>,
    ) -> Result<(RawCatalog, HashSet<u32>), PffError> {
        let mut catalog = RawCatalog::default();
        let mut visited_messages = HashSet::new();
        let root_address = FolderAddress::root();
        let root_unit = RecoveryUnit::Folder {
            address: root_address,
        };
        if skipped.contains(&root_unit) {
            catalog.record_issue(CatalogIssue {
                node_id: None,
                operation: "skip isolated recovery unit",
                message: "root folder subtree was isolated".to_owned(),
            });
            return Ok((catalog, visited_messages));
        }
        emit(
            sink,
            "start recovery unit",
            CatalogEvent::UnitStart(root_unit),
        )?;
        let root = self.root_folder()?;
        let mut folders = vec![(root, None, root_address, true)];

        while let Some((folder, parent_id, folder_address, unit_started)) = folders.pop() {
            let folder_unit = RecoveryUnit::Folder {
                address: folder_address,
            };
            if !unit_started {
                if skipped.contains(&folder_unit) {
                    catalog.record_issue(CatalogIssue {
                        node_id: parent_id,
                        operation: "skip isolated recovery unit",
                        message: "folder subtree was isolated".to_owned(),
                    });
                    continue;
                }
                emit(
                    sink,
                    "start recovery unit",
                    CatalogEvent::UnitStart(folder_unit),
                )?;
            }
            let folder_id = match folder.identifier() {
                Ok(folder_id) => folder_id,
                Err(error) => {
                    record_item_issue(&mut catalog, None, "get folder identifier", error)?;
                    emit(
                        sink,
                        "end recovery unit",
                        CatalogEvent::UnitEnd(folder_unit),
                    )?;
                    continue;
                }
            };
            catalog.folders = checked_increment(catalog.folders, "folder count", 1_000_000)?;
            let name = match folder.folder_name() {
                Ok(name) => name,
                Err(error) => {
                    record_item_issue(&mut catalog, Some(folder_id), "get folder name", error)?;
                    None
                }
            };
            let container_class = recover_item_value(
                &mut catalog,
                Some(folder_id),
                "get folder container class",
                folder.message_string(0x3613),
            )?;
            emit(
                sink,
                "folder metadata",
                CatalogEvent::Folder {
                    id: folder_id,
                    parent_id,
                    name,
                    container_class,
                },
            )?;
            if let Err(error) = stream_item_properties(
                &folder,
                PropertyOwner::Folder(folder_id),
                sink,
                &mut catalog,
            ) {
                record_item_issue(
                    &mut catalog,
                    Some(folder_id),
                    "stream folder properties",
                    error,
                )?;
            }

            let message_count = match folder
                .sub_message_count()
                .and_then(|count| bounded_folder_item_count(count, "folder message count"))
            {
                Ok(count) => count,
                Err(error) => {
                    record_item_issue(
                        &mut catalog,
                        Some(folder_id),
                        "count folder messages",
                        error,
                    )?;
                    0
                }
            };
            let associated_count = match folder.sub_associated_count().and_then(|count| {
                bounded_folder_item_count(count, "folder associated contents count")
            }) {
                Ok(count) => count,
                Err(error) => {
                    record_item_issue(
                        &mut catalog,
                        Some(folder_id),
                        "count folder associated contents",
                        error,
                    )?;
                    0
                }
            };
            let child_count = match folder.sub_folder_count() {
                Ok(count) => count,
                Err(error) => {
                    record_item_issue(&mut catalog, Some(folder_id), "count child folders", error)?;
                    0
                }
            };
            emit(
                sink,
                "end recovery unit",
                CatalogEvent::UnitEnd(folder_unit),
            )?;
            for index in (0..child_count).rev() {
                let child_index = u32::try_from(index).map_err(|_| PffError::LimitExceeded {
                    field: "child folder index",
                    value: index,
                    limit: u64::from(u32::MAX),
                })?;
                let Some(child_address) = folder_address.child(child_index) else {
                    catalog.record_issue(CatalogIssue {
                        node_id: Some(folder_id),
                        operation: "traverse child folder",
                        message: format!("folder depth exceeds {MAX_FOLDER_DEPTH}"),
                    });
                    continue;
                };
                let child_unit = RecoveryUnit::ChildFolder {
                    address: child_address,
                };
                if skipped.contains(&child_unit) {
                    catalog.record_issue(CatalogIssue {
                        node_id: Some(folder_id),
                        operation: "skip isolated recovery unit",
                        message: format!("child folder index {index} was isolated"),
                    });
                    continue;
                }
                emit(
                    sink,
                    "start recovery unit",
                    CatalogEvent::UnitStart(child_unit),
                )?;
                match folder.sub_folder(index) {
                    Ok(child) => {
                        folders.push((child, Some(folder_id), child_address, false));
                    }
                    Err(error) => record_item_issue(
                        &mut catalog,
                        Some(folder_id),
                        "read child folder",
                        error,
                    )?,
                }
                emit(sink, "end recovery unit", CatalogEvent::UnitEnd(child_unit))?;
            }
            for index in 0..message_count {
                let unit = RecoveryUnit::Normal {
                    folder: folder_address,
                    folder_id,
                    message_index: index,
                };
                if skipped.contains(&unit) {
                    catalog.record_issue(CatalogIssue {
                        node_id: Some(folder_id),
                        operation: "skip isolated recovery unit",
                        message: format!("normal message index {index} was isolated"),
                    });
                    continue;
                }
                emit(sink, "start recovery unit", CatalogEvent::UnitStart(unit))?;
                let item = match folder.sub_message(index) {
                    Ok(item) => item,
                    Err(error) => {
                        record_item_issue(
                            &mut catalog,
                            Some(folder_id),
                            "read folder message",
                            error,
                        )?;
                        emit(sink, "end recovery unit", CatalogEvent::UnitEnd(unit))?;
                        continue;
                    }
                };
                let mut messages = vec![MessageWork {
                    item,
                    folder_id: Some(folder_id),
                    parent_message_id: None,
                    parent_attachment_index: None,
                    embedded_path: Vec::new(),
                    depth: 0,
                    provenance: CatalogProvenance::Normal,
                    recovery_index: None,
                    associated: false,
                }];
                while let Some(work) = messages.pop() {
                    if let Err(error) = process_message(
                        work,
                        &mut messages,
                        &mut visited_messages,
                        sink,
                        &mut catalog,
                    ) {
                        match error {
                            error @ PffError::Sink { .. } => return Err(error),
                            error => record_item_issue(
                                &mut catalog,
                                None,
                                "process folder message",
                                error,
                            )?,
                        }
                    }
                }
                emit(sink, "end recovery unit", CatalogEvent::UnitEnd(unit))?;
            }
            for index in 0..associated_count {
                let unit = RecoveryUnit::Associated {
                    folder: folder_address,
                    folder_id,
                    message_index: index,
                };
                if skipped.contains(&unit) {
                    catalog.record_issue(CatalogIssue {
                        node_id: Some(folder_id),
                        operation: "skip isolated recovery unit",
                        message: format!("associated content index {index} was isolated"),
                    });
                    continue;
                }
                emit(sink, "start recovery unit", CatalogEvent::UnitStart(unit))?;
                let item = match folder.sub_associated(index) {
                    Ok(item) => item,
                    Err(error) => {
                        record_item_issue(
                            &mut catalog,
                            Some(folder_id),
                            "read folder associated content",
                            error,
                        )?;
                        emit(sink, "end recovery unit", CatalogEvent::UnitEnd(unit))?;
                        continue;
                    }
                };
                let mut messages = vec![MessageWork {
                    item,
                    folder_id: Some(folder_id),
                    parent_message_id: None,
                    parent_attachment_index: None,
                    embedded_path: Vec::new(),
                    depth: 0,
                    provenance: CatalogProvenance::Normal,
                    recovery_index: None,
                    associated: true,
                }];
                while let Some(work) = messages.pop() {
                    if let Err(error) = process_message(
                        work,
                        &mut messages,
                        &mut visited_messages,
                        sink,
                        &mut catalog,
                    ) {
                        match error {
                            error @ PffError::Sink { .. } => return Err(error),
                            error => record_item_issue(
                                &mut catalog,
                                None,
                                "process folder associated content",
                                error,
                            )?,
                        }
                    }
                }
                emit(sink, "end recovery unit", CatalogEvent::UnitEnd(unit))?;
            }
        }
        Ok((catalog, visited_messages))
    }

    fn stream_recovery_collection(
        &self,
        provenance: CatalogProvenance,
        functions: RecoveryCollectionFunctions,
        visited_messages: &mut HashSet<u32>,
        sink: &mut dyn CatalogSink,
        catalog: &mut RawCatalog,
        skipped: &HashSet<RecoveryUnit>,
    ) -> Result<(), PffError> {
        let mut count = 0_i32;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is open and count/error are initialized out-pointers.
        let result = unsafe { (functions.count)(self.raw, &mut count, &mut error) };
        check_one(result, error, "count recovery items")?;
        if count < 0 || u64::try_from(count).unwrap_or(u64::MAX) > MAX_MESSAGES {
            return Err(PffError::InvalidValue {
                field: "recovery item count",
                value: i64::from(count),
            });
        }
        for index in 0..count {
            let recovery_index = u64::try_from(index).map_err(|_| PffError::InvalidValue {
                field: "recovery item index",
                value: i64::from(index),
            })?;
            let unit = match provenance {
                CatalogProvenance::Recovered => RecoveryUnit::Recovered {
                    index: recovery_index,
                },
                CatalogProvenance::Orphan => RecoveryUnit::Orphan {
                    index: recovery_index,
                },
                CatalogProvenance::Fragment => RecoveryUnit::Fragment {
                    index: recovery_index,
                },
                CatalogProvenance::Normal => {
                    return Err(PffError::InvalidValue {
                        field: "recovery provenance",
                        value: 0,
                    });
                }
            };
            if skipped.contains(&unit) {
                catalog.record_issue(CatalogIssue {
                    node_id: None,
                    operation: "skip isolated recovery unit",
                    message: format!("recovery item index {recovery_index} was isolated"),
                });
                continue;
            }
            emit(sink, "start recovery unit", CatalogEvent::UnitStart(unit))?;
            let mut raw = ptr::null_mut();
            error = ptr::null_mut();
            // SAFETY: the index is within the count returned by libpff and outputs are initialized.
            let result = unsafe { (functions.item)(self.raw, index, &mut raw, &mut error) };
            if let Err(error) = check_one(result, error, "get recovery item") {
                if let Ok(item) = PffItem::from_raw(raw, "clean failed recovery item") {
                    drop(item);
                }
                catalog.record_issue(catalog_issue(None, "get recovery item", error));
                emit(sink, "end recovery unit", CatalogEvent::UnitEnd(unit))?;
                continue;
            }
            let item = match PffItem::from_raw(raw, "get recovery item") {
                Ok(item) => item,
                Err(error) => {
                    catalog.record_issue(catalog_issue(None, "get recovery item", error));
                    emit(sink, "end recovery unit", CatalogEvent::UnitEnd(unit))?;
                    continue;
                }
            };
            let mut pending = vec![MessageWork {
                item,
                folder_id: None,
                parent_message_id: None,
                parent_attachment_index: None,
                embedded_path: Vec::new(),
                depth: 0,
                provenance,
                recovery_index: Some(recovery_index),
                associated: false,
            }];
            while let Some(work) = pending.pop() {
                if let Err(error) =
                    process_message(work, &mut pending, visited_messages, sink, catalog)
                {
                    match error {
                        error @ PffError::Sink { .. } => return Err(error),
                        error => record_item_issue(catalog, None, "process recovery item", error)?,
                    }
                }
            }
            emit(sink, "end recovery unit", CatalogEvent::UnitEnd(unit))?;
        }
        Ok(())
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

fn bounded_folder_item_count(count: u64, field: &'static str) -> Result<u64, PffError> {
    if count > MAX_MESSAGES {
        Err(PffError::LimitExceeded {
            field,
            value: count,
            limit: MAX_MESSAGES,
        })
    } else {
        Ok(count)
    }
}

fn recovery_flags(mode: RecoveryMode) -> u8 {
    match mode {
        RecoveryMode::Balanced => 0,
        RecoveryMode::Aggressive => {
            RECOVERY_FLAG_IGNORE_ALLOCATION_DATA | RECOVERY_FLAG_SCAN_FOR_FRAGMENTS
        }
    }
}

fn recovered_provenance(mode: RecoveryMode) -> CatalogProvenance {
    let _ = mode;
    CatalogProvenance::Recovered
}

struct MessageWork {
    item: PffItem,
    folder_id: Option<u32>,
    parent_message_id: Option<u32>,
    parent_attachment_index: Option<u32>,
    embedded_path: Vec<u32>,
    depth: u32,
    provenance: CatalogProvenance,
    recovery_index: Option<u64>,
    associated: bool,
}

fn process_message(
    work: MessageWork,
    pending: &mut Vec<MessageWork>,
    visited: &mut HashSet<u32>,
    sink: &mut dyn CatalogSink,
    catalog: &mut RawCatalog,
) -> Result<(), PffError> {
    let issue_state = (catalog.issues.len(), catalog.issues_dropped);
    let message_id = recover_item_value(
        catalog,
        None,
        "get message identifier",
        work.item.identifier(),
    )?;
    if stable_top_level_identifier_seen(visited, message_id, &work.embedded_path) {
        catalog.record_issue(CatalogIssue {
            node_id: Some(message_id),
            operation: "traverse message",
            message: "duplicate message identifier".to_owned(),
        });
        return Ok(());
    }
    let messages = checked_increment(catalog.messages, "message count", MAX_MESSAGES)?;
    let recovered_messages = match work.provenance {
        CatalogProvenance::Recovered => checked_increment(
            catalog.recovered_messages,
            "recovered message count",
            MAX_MESSAGES,
        )?,
        _ => catalog.recovered_messages,
    };
    let orphan_messages = match work.provenance {
        CatalogProvenance::Orphan => checked_increment(
            catalog.orphan_messages,
            "orphan message count",
            MAX_MESSAGES,
        )?,
        _ => catalog.orphan_messages,
    };
    let fragment_messages = match work.provenance {
        CatalogProvenance::Fragment => checked_increment(
            catalog.fragment_messages,
            "fragment message count",
            MAX_MESSAGES,
        )?,
        _ => catalog.fragment_messages,
    };
    let embedded_messages = if work.parent_message_id.is_some() {
        checked_increment(
            catalog.embedded_messages,
            "embedded message count",
            u64::MAX,
        )?
    } else {
        catalog.embedded_messages
    };
    let message_class = recover_item_value(
        catalog,
        Some(message_id),
        "get message class",
        work.item.message_string(MESSAGE_CLASS),
    )?;
    let recipient_count_hint = recover_item_value(
        catalog,
        Some(message_id),
        "get recipient count",
        work.item.optional_u32(RECIPIENT_COUNT),
    )?;
    // Catalog support is a class/intake capability. Parent class and
    // attachment-linkage validity are enforced after the full graph is durable.
    let supported = message_class
        .as_deref()
        .is_some_and(|value| supported_message_class(value, work.parent_message_id.is_some()));
    let unsupported_messages = if supported {
        catalog.unsupported_messages
    } else {
        checked_increment(
            catalog.unsupported_messages,
            "unsupported message count",
            u64::MAX,
        )?
    };
    let item_type = recover_item_value(
        catalog,
        Some(message_id),
        "get message item type",
        work.item.item_type(),
    )?;
    let subject = recover_item_value(
        catalog,
        Some(message_id),
        "get message subject",
        work.item.message_string(SUBJECT),
    )?;
    let sender_name = recover_item_value(
        catalog,
        Some(message_id),
        "get sender name",
        work.item.message_string(SENDER_NAME),
    )?;
    let sender_email = recover_item_value(
        catalog,
        Some(message_id),
        "get sender email",
        work.item.message_string(SENDER_EMAIL),
    )?;
    let submit_filetime = recover_item_value(
        catalog,
        Some(message_id),
        "get client submit time",
        work.item.optional_filetime(
            "get client submit time",
            bindings::libpff_message_get_client_submit_time,
        ),
    )?;
    let submit_filetime = contain_filetime(
        catalog,
        message_id,
        "get client submit time",
        submit_filetime,
    );
    let delivery_filetime = recover_item_value(
        catalog,
        Some(message_id),
        "get delivery time",
        work.item.optional_filetime(
            "get delivery time",
            bindings::libpff_message_get_delivery_time,
        ),
    )?;
    let delivery_filetime =
        contain_filetime(catalog, message_id, "get delivery time", delivery_filetime);
    emit(
        sink,
        "message metadata",
        CatalogEvent::MessageStart {
            id: message_id,
            provenance: work.provenance,
            recovery_index: work.recovery_index,
            folder_id: work.folder_id,
            parent_message_id: work.parent_message_id,
            parent_attachment_index: work.parent_attachment_index,
            embedded_path: work.embedded_path.clone(),
            associated: work.associated,
            item_type,
            message_class,
            subject,
            sender_name,
            sender_email,
            submit_filetime,
            delivery_filetime,
            supported,
        },
    )?;
    catalog.messages = messages;
    catalog.recovered_messages = recovered_messages;
    catalog.orphan_messages = orphan_messages;
    catalog.fragment_messages = fragment_messages;
    catalog.embedded_messages = embedded_messages;
    catalog.unsupported_messages = unsupported_messages;
    mark_stable_top_level_identifier(visited, message_id, &work.embedded_path);
    if let Err(error) =
        stream_recipients(&work.item, message_id, recipient_count_hint, sink, catalog)
    {
        record_item_issue(catalog, Some(message_id), "stream recipients", error)?;
    }
    match work.item.attachment_count() {
        Ok(count) => {
            if let Err(error) =
                stream_attachments(&work, message_id, count, pending, visited, sink, catalog)
            {
                record_attachment_issue(catalog, Some(message_id), "stream attachments", error)?;
            }
        }
        Err(error) => {
            record_attachment_issue(catalog, Some(message_id), "count attachments", error)?;
        }
    }
    if let Err(error) = stream_item_properties(
        &work.item,
        PropertyOwner::Message(message_id),
        sink,
        catalog,
    ) {
        record_item_issue(
            catalog,
            Some(message_id),
            "stream message properties",
            error,
        )?;
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

fn supported_message_class(value: &str, embedded: bool) -> bool {
    !value.is_empty() && (!calendar_exception_message_class(value) || embedded)
}

fn calendar_exception_message_class(value: &str) -> bool {
    value.eq_ignore_ascii_case("IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}")
}

fn stable_top_level_identifier_seen(
    visited: &HashSet<u32>,
    identifier: u32,
    embedded_path: &[u32],
) -> bool {
    embedded_path.is_empty() && identifier != 0 && visited.contains(&identifier)
}

fn mark_stable_top_level_identifier(
    visited: &mut HashSet<u32>,
    identifier: u32,
    embedded_path: &[u32],
) {
    if embedded_path.is_empty() && identifier != 0 {
        visited.insert(identifier);
    }
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
    count: u64,
    pending: &mut Vec<MessageWork>,
    visited: &mut HashSet<u32>,
    sink: &mut dyn CatalogSink,
    catalog: &mut RawCatalog,
) -> Result<(), PffError> {
    for index in 0..count {
        let index_u32 = checked_index(index, "attachment index")?;
        let attachment = match work.item.attachment(index) {
            Ok(attachment) => attachment,
            Err(error) => {
                emit(
                    sink,
                    "attachment metadata",
                    CatalogEvent::AttachmentStart {
                        message_id,
                        index: index_u32,
                        attachment_type: None,
                        data_size: None,
                        filename: None,
                    },
                )?;
                catalog.attachments =
                    checked_increment(catalog.attachments, "attachment count", u64::MAX)?;
                emit(
                    sink,
                    "attachment abort",
                    CatalogEvent::AttachmentAbort {
                        message_id,
                        index: index_u32,
                    },
                )?;
                record_attachment_issue(catalog, Some(message_id), "get attachment", error)?;
                continue;
            }
        };
        let attachment_method = recover_attachment_value(
            catalog,
            message_id,
            "get attachment method",
            attachment
                .first_record_set()
                .and_then(|set| set.map(|set| set.optional_u32(ATTACH_METHOD)).transpose())
                .map(Option::flatten),
        )?;
        let reference_attachment = matches!(attachment_method, Some(2 | 3 | 4 | 7));
        let attachment_type = if reference_attachment {
            Some(i32::from(b'r'))
        } else {
            recover_attachment_value(
                catalog,
                message_id,
                "get attachment type",
                attachment.attachment_type(),
            )?
        };
        let data_size = if reference_attachment {
            None
        } else {
            recover_attachment_value(
                catalog,
                message_id,
                "get attachment size",
                attachment.attachment_size(),
            )?
        };
        let filename = recover_attachment_value(
            catalog,
            message_id,
            "get attachment filename",
            attachment.first_record_set().and_then(|set| {
                set.map(|set| {
                    set.optional_string(ATTACH_FILENAME).and_then(|value| {
                        if value.is_some() {
                            Ok(value)
                        } else {
                            set.optional_string(ATTACH_FILENAME_SHORT)
                        }
                    })
                })
                .transpose()
                .map(Option::flatten)
            }),
        )?;
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
        let traversal_order = sink.traversal_order();
        let writer_order = traversal_order == TraversalOrder::Writer;
        let embedded_first = traversal_order != TraversalOrder::Source;
        if !embedded_first {
            stream_attachment_properties(&attachment, message_id, index_u32, sink, catalog)?;
        }
        let embedded_attachment = attachment_type == Some(i32::from(b'i'));
        if !embedded_attachment && !reference_attachment {
            let result = data_size
                .ok_or(PffError::Native {
                    operation: "get attachment size",
                    detail: "attachment data size is unavailable".to_owned(),
                })
                .and_then(|expected| {
                    attachment.stream_attachment(message_id, index_u32, expected, sink)
                });
            let actual = match result {
                Ok(actual) => actual,
                Err(error @ PffError::Sink { .. }) => return Err(error),
                Err(error) => {
                    if embedded_first {
                        stream_attachment_properties(
                            &attachment,
                            message_id,
                            index_u32,
                            sink,
                            catalog,
                        )?;
                    }
                    emit(
                        sink,
                        "attachment abort",
                        CatalogEvent::AttachmentAbort {
                            message_id,
                            index: index_u32,
                        },
                    )?;
                    record_attachment_issue(
                        catalog,
                        Some(message_id),
                        "stream attachment data",
                        error,
                    )?;
                    continue;
                }
            };
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
        if embedded_attachment {
            let embedded = match attachment
                .optional_item("get embedded item", bindings::libpff_attachment_get_item)
                .and_then(|item| {
                    item.ok_or(PffError::Native {
                        operation: "get embedded item",
                        detail: "embedded attachment item is unavailable".to_owned(),
                    })
                }) {
                Ok(embedded) => embedded,
                Err(error @ PffError::Sink { .. }) => return Err(error),
                Err(error) => {
                    if embedded_first {
                        stream_attachment_properties(
                            &attachment,
                            message_id,
                            index_u32,
                            sink,
                            catalog,
                        )?;
                    }
                    emit(
                        sink,
                        "attachment abort",
                        CatalogEvent::AttachmentAbort {
                            message_id,
                            index: index_u32,
                        },
                    )?;
                    record_attachment_issue(catalog, Some(message_id), "get embedded item", error)?;
                    continue;
                }
            };
            let embedded_work = (|| -> Result<MessageWork, PffError> {
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
                let mut embedded_path = work.embedded_path.clone();
                embedded_path.push(index_u32);
                Ok(MessageWork {
                    item: embedded,
                    folder_id: work.folder_id,
                    parent_message_id: Some(message_id),
                    parent_attachment_index: Some(index_u32),
                    embedded_path,
                    depth,
                    provenance: work.provenance,
                    recovery_index: work.recovery_index,
                    associated: false,
                })
            })();
            match embedded_work {
                Ok(embedded_work) if writer_order => {
                    process_message(embedded_work, pending, visited, sink, catalog)?;
                }
                Ok(embedded_work) => pending.push(embedded_work),
                Err(error) => {
                    if embedded_first {
                        stream_attachment_properties(
                            &attachment,
                            message_id,
                            index_u32,
                            sink,
                            catalog,
                        )?;
                    }
                    emit(
                        sink,
                        "attachment abort",
                        CatalogEvent::AttachmentAbort {
                            message_id,
                            index: index_u32,
                        },
                    )?;
                    record_attachment_issue(
                        catalog,
                        Some(message_id),
                        "queue embedded item",
                        error,
                    )?;
                    continue;
                }
            }
        }
        if embedded_first {
            stream_attachment_properties(&attachment, message_id, index_u32, sink, catalog)?;
        }
        emit(
            sink,
            "attachment end",
            CatalogEvent::AttachmentEnd {
                message_id,
                index: index_u32,
            },
        )?;
    }
    Ok(())
}

fn stream_attachment_properties(
    attachment: &PffItem,
    message_id: u32,
    index: u32,
    sink: &mut dyn CatalogSink,
    catalog: &mut RawCatalog,
) -> Result<(), PffError> {
    if let Err(error) = stream_item_properties(
        attachment,
        PropertyOwner::Attachment { message_id, index },
        sink,
        catalog,
    ) {
        record_attachment_issue(
            catalog,
            Some(message_id),
            "stream attachment properties",
            error,
        )?;
    }
    Ok(())
}

fn recover_attachment_value<T: Default>(
    catalog: &mut RawCatalog,
    message_id: u32,
    operation: &'static str,
    result: Result<T, PffError>,
) -> Result<T, PffError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            record_attachment_issue(catalog, Some(message_id), operation, error)?;
            Ok(T::default())
        }
    }
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
        if let Some(identity) = entry.named_property_identity()? {
            emit(
                sink,
                "named property identity",
                CatalogEvent::NamedProperty {
                    descriptor,
                    identity,
                },
            )?;
        }
        emit(
            sink,
            "property start",
            CatalogEvent::PropertyStart(descriptor),
        )?;
        let request = sink.property_payload(descriptor);
        let actual = match entry.stream(descriptor, sink) {
            Ok(actual) => actual,
            Err(error @ PffError::Sink { .. }) => return Err(error),
            Err(error) => {
                let reason = error.to_string();
                emit(
                    sink,
                    "property abort",
                    CatalogEvent::PropertyAbort { descriptor, reason },
                )?;
                return Err(error);
            }
        };
        let expected = match request {
            PayloadRequest::Full => descriptor.data_size,
            PayloadRequest::Prefix(limit) => descriptor.data_size.min(limit),
        };
        if actual != expected {
            let error = PffError::StreamSizeMismatch {
                field: "property data",
                expected,
                actual,
            };
            emit(
                sink,
                "property abort",
                CatalogEvent::PropertyAbort {
                    descriptor,
                    reason: error.to_string(),
                },
            )?;
            return Err(error);
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

    fn named_property_identity(&self) -> Result<Option<NamedPropertyIdentity>, PffError> {
        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and the borrowed map-entry output is initialized.
        let result = unsafe {
            bindings::libpff_record_entry_get_name_to_id_map_entry(self.raw, &mut raw, &mut error)
        };
        match result {
            0 => {
                free_error(error);
                return Ok(None);
            }
            1 => free_error(error),
            _ => return Err(native_error(error, "get named property identity")),
        }
        if raw.is_null() {
            return Err(PffError::NullPointer {
                operation: "get named property identity",
            });
        }
        let mut guid = [0_u8; 16];
        error = ptr::null_mut();
        // SAFETY: raw remains owned by the live record entry and guid has exactly 16 bytes.
        let result = unsafe {
            bindings::libpff_name_to_id_map_entry_get_guid(
                raw,
                guid.as_mut_ptr(),
                guid.len(),
                &mut error,
            )
        };
        check_one(result, error, "get named property GUID")?;

        let mut entry_type = 0_u8;
        error = ptr::null_mut();
        // SAFETY: raw remains valid and entry_type is initialized.
        let result = unsafe {
            bindings::libpff_name_to_id_map_entry_get_type(raw, &mut entry_type, &mut error)
        };
        check_one(result, error, "get named property kind")?;
        let name = match entry_type {
            b'n' => {
                let mut number = 0_u32;
                error = ptr::null_mut();
                // SAFETY: raw remains valid and number is initialized.
                let result = unsafe {
                    bindings::libpff_name_to_id_map_entry_get_number(raw, &mut number, &mut error)
                };
                check_one(result, error, "get named property number")?;
                NamedPropertyName::Numeric(number)
            }
            b's' => {
                let value = read_optional_identity_string(
                    raw,
                    "get named property string size",
                    "get named property string",
                    bindings::libpff_name_to_id_map_entry_get_utf8_string_size,
                    bindings::libpff_name_to_id_map_entry_get_utf8_string,
                )?
                .ok_or(PffError::Native {
                    operation: "get named property string",
                    detail: "value is absent".to_owned(),
                })?;
                NamedPropertyName::String(value)
            }
            value => {
                return Err(PffError::InvalidValue {
                    field: "named property kind",
                    value: i64::from(value),
                });
            }
        };
        Ok(Some(NamedPropertyIdentity { guid, name }))
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
        let request = sink.property_payload(descriptor);
        let limit = match request {
            PayloadRequest::Full => descriptor.data_size,
            PayloadRequest::Prefix(limit) => descriptor.data_size.min(limit),
        };
        let mut total = 0_u64;
        let mut buffer = vec![0_u8; STREAM_CHUNK_BYTES];
        loop {
            if matches!(request, PayloadRequest::Prefix(_)) && total >= limit {
                break;
            }
            let requested = match request {
                PayloadRequest::Full => buffer.len(),
                PayloadRequest::Prefix(_) => usize::try_from(
                    limit
                        .saturating_sub(total)
                        .min(u64::try_from(buffer.len()).unwrap_or(u64::MAX)),
                )
                .map_err(|_| PffError::InvalidValue {
                    field: "record read size",
                    value: i64::MAX,
                })?,
            };
            let mut error = ptr::null_mut();
            // SAFETY: self.raw is valid and buffer is writable for its declared size.
            let read = unsafe {
                bindings::libpff_record_entry_read_buffer(
                    self.raw,
                    buffer.as_mut_ptr(),
                    requested,
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

    fn sub_associated(&self, index: u64) -> Result<Self, PffError> {
        let index = native_index(index, "associated content index")?;
        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and outputs are initialized.
        let result = unsafe {
            bindings::libpff_folder_get_sub_associated_content(
                self.raw, index, &mut raw, &mut error,
            )
        };
        check_one(result, error, "get sub associated content")?;
        Self::from_raw(raw, "get sub associated content")
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
        expected: u64,
        sink: &mut dyn CatalogSink,
    ) -> Result<u64, PffError> {
        let request = sink.attachment_payload(message_id, index, Some(expected));
        let limit = match request {
            PayloadRequest::Full => expected,
            PayloadRequest::Prefix(limit) => expected.min(limit),
        };
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
            if matches!(request, PayloadRequest::Prefix(_)) && total >= limit {
                break;
            }
            let requested = match request {
                PayloadRequest::Full => buffer.len(),
                PayloadRequest::Prefix(_) => usize::try_from(
                    limit
                        .saturating_sub(total)
                        .min(u64::try_from(buffer.len()).unwrap_or(u64::MAX)),
                )
                .map_err(|_| PffError::InvalidValue {
                    field: "attachment read size",
                    value: i64::MAX,
                })?,
            };
            error = ptr::null_mut();
            // SAFETY: self.raw is valid and buffer is writable for its declared size.
            let read = unsafe {
                bindings::libpff_attachment_data_read_buffer(
                    self.raw,
                    buffer.as_mut_ptr(),
                    requested,
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
            let read_u64 = u64::try_from(read).map_err(|_| PffError::InvalidValue {
                field: "attachment read size",
                value: i64::MAX,
            })?;
            let observed = total.checked_add(read_u64).ok_or(PffError::LimitExceeded {
                field: "attachment streamed size",
                value: u64::MAX,
                limit: u64::MAX - 1,
            })?;
            let remaining = limit.saturating_sub(total);
            let emitted =
                usize::try_from(remaining.min(read_u64)).map_err(|_| PffError::InvalidValue {
                    field: "attachment read size",
                    value: i64::MAX,
                })?;
            if emitted != 0 {
                emit(
                    sink,
                    "attachment data",
                    CatalogEvent::AttachmentData {
                        message_id,
                        index,
                        bytes: &buffer[..emitted],
                    },
                )?;
                let emitted_u64 = u64::try_from(emitted).map_err(|_| PffError::InvalidValue {
                    field: "attachment read size",
                    value: i64::MAX,
                })?;
                total = total
                    .checked_add(emitted_u64)
                    .ok_or(PffError::LimitExceeded {
                        field: "attachment streamed size",
                        value: u64::MAX,
                        limit: u64::MAX - 1,
                    })?;
            }
            if observed > limit {
                return Err(PffError::StreamSizeMismatch {
                    field: "attachment data",
                    expected: limit,
                    actual: observed,
                });
            }
        }
        if total == limit {
            Ok(total)
        } else {
            Err(PffError::StreamSizeMismatch {
                field: "attachment data",
                expected: limit,
                actual: total,
            })
        }
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
    Ok(
        read_optional_string_bytes(raw, size_operation, read_operation, size_fn, read_fn)?
            .map(decode_native_string),
    )
}

fn read_optional_identity_string<T>(
    raw: *mut T,
    size_operation: &'static str,
    read_operation: &'static str,
    size_fn: unsafe extern "C" fn(*mut T, *mut usize, *mut *mut libpff_error_t) -> i32,
    read_fn: unsafe extern "C" fn(*mut T, *mut u8, usize, *mut *mut libpff_error_t) -> i32,
) -> Result<Option<String>, PffError> {
    read_optional_string_bytes(raw, size_operation, read_operation, size_fn, read_fn)?
        .map(decode_native_identity_string)
        .transpose()
}

fn read_optional_string_bytes<T>(
    raw: *mut T,
    size_operation: &'static str,
    read_operation: &'static str,
    size_fn: unsafe extern "C" fn(*mut T, *mut usize, *mut *mut libpff_error_t) -> i32,
    read_fn: unsafe extern "C" fn(*mut T, *mut u8, usize, *mut *mut libpff_error_t) -> i32,
) -> Result<Option<Vec<u8>>, PffError> {
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
    Ok(Some(bytes))
}

fn decode_native_string(mut bytes: Vec<u8>) -> String {
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn decode_native_identity_string(mut bytes: Vec<u8>) -> Result<String, PffError> {
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    String::from_utf8(bytes).map_err(|error| PffError::Native {
        operation: "decode named property string",
        detail: error.to_string(),
    })
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

fn record_item_issue(
    catalog: &mut RawCatalog,
    node_id: Option<u32>,
    operation: &'static str,
    error: PffError,
) -> Result<(), PffError> {
    match error {
        error @ (PffError::Sink { .. } | PffError::LimitExceeded { .. }) => Err(error),
        error => {
            catalog.record_issue(catalog_issue(node_id, operation, error));
            Ok(())
        }
    }
}

fn record_attachment_issue(
    catalog: &mut RawCatalog,
    node_id: Option<u32>,
    operation: &'static str,
    error: PffError,
) -> Result<(), PffError> {
    match error {
        error @ PffError::Sink { .. } => Err(error),
        error => {
            catalog.record_issue(catalog_issue(node_id, operation, error));
            Ok(())
        }
    }
}

fn recover_item_value<T: Default>(
    catalog: &mut RawCatalog,
    node_id: Option<u32>,
    operation: &'static str,
    result: Result<T, PffError>,
) -> Result<T, PffError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            record_item_issue(catalog, node_id, operation, error)?;
            Ok(T::default())
        }
    }
}

fn contain_filetime(
    catalog: &mut RawCatalog,
    message_id: u32,
    operation: &'static str,
    value: Option<u64>,
) -> Option<u64> {
    const MAX_WRITABLE_FILETIME: u64 = 9_223_372_036_854_775_807;
    match value {
        Some(value) if value > MAX_WRITABLE_FILETIME => {
            catalog.record_issue(CatalogIssue {
                node_id: Some(message_id),
                operation,
                message: format!(
                    "FILETIME exceeds the writable signed range: {value} > {MAX_WRITABLE_FILETIME}"
                ),
            });
            Some(value)
        }
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        CatalogEvent, CatalogIssue, CatalogProvenance, CatalogSink, FolderAddress,
        MAX_CATALOG_ISSUES, PropertyDescriptor, PropertyOwner,
        RECOVERY_FLAG_IGNORE_ALLOCATION_DATA, RECOVERY_FLAG_SCAN_FOR_FRAGMENTS, RawCatalog,
        RecoveryMode, RecoveryUnit, STREAM_CHUNK_BYTES, checked_increment, contain_filetime,
        decode_native_identity_string, decode_native_string, mark_stable_top_level_identifier,
        record_attachment_issue, record_item_issue, recover_item_value, recovered_provenance,
        recovery_flags, stable_top_level_identifier_seen, supported_message_class,
        validate_string_size,
    };
    use crate::PffError;

    #[test]
    fn arbitrary_classes_are_supported_but_exact_calendar_exception_must_be_embedded() {
        assert!(supported_message_class(
            "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}",
            true
        ));
        assert!(supported_message_class(
            "ipm.ole.class.{00061055-0000-0000-c000-000000000046}",
            true
        ));
        assert!(!supported_message_class(
            "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}",
            false
        ));
        assert!(supported_message_class(
            "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}.Custom",
            true
        ));
        assert!(supported_message_class(
            "IPM.OLE.CLASS.{00061056-0000-0000-C000-000000000046}",
            true
        ));
        assert!(!supported_message_class("", true));
    }

    #[test]
    fn named_property_identity_rejects_invalid_utf8() {
        assert!(matches!(
            decode_native_identity_string(b"Checkpoint\0".to_vec()),
            Ok(value) if value == "Checkpoint"
        ));
        assert!(matches!(
            decode_native_identity_string(vec![0xff, 0]),
            Err(PffError::Native {
                operation: "decode named property string",
                ..
            })
        ));
    }

    #[test]
    fn aggressive_mode_uses_both_native_flags_without_relabeling_generic_recovery_items() {
        assert_eq!(recovery_flags(RecoveryMode::Balanced), 0);
        assert_eq!(
            recovery_flags(RecoveryMode::Aggressive),
            RECOVERY_FLAG_IGNORE_ALLOCATION_DATA | RECOVERY_FLAG_SCAN_FOR_FRAGMENTS
        );
        assert_eq!(
            recovered_provenance(RecoveryMode::Balanced),
            CatalogProvenance::Recovered
        );
        assert_eq!(
            recovered_provenance(RecoveryMode::Aggressive),
            CatalogProvenance::Recovered
        );
    }

    #[test]
    fn unwritable_filetime_is_candidate_local_damage() {
        let mut catalog = RawCatalog::default();
        assert_eq!(
            contain_filetime(
                &mut catalog,
                7,
                "submit time",
                Some(9_223_372_036_854_775_807)
            ),
            Some(9_223_372_036_854_775_807)
        );
        assert!(catalog.issues.is_empty());
        assert_eq!(
            contain_filetime(&mut catalog, 7, "submit time", Some(u64::MAX)),
            Some(u64::MAX)
        );
        assert_eq!(catalog.issues.len(), 1);
        assert_eq!(catalog.issues[0].node_id, Some(7));
    }

    #[test]
    fn stable_identity_dedup_applies_only_to_nonzero_top_level_messages() {
        let mut visited = HashSet::new();
        assert!(!stable_top_level_identifier_seen(&visited, 42, &[]));
        assert!(!stable_top_level_identifier_seen(&visited, 0, &[]));
        mark_stable_top_level_identifier(&mut visited, 0, &[]);
        assert!(visited.is_empty());
        mark_stable_top_level_identifier(&mut visited, 42, &[]);
        assert!(stable_top_level_identifier_seen(&visited, 42, &[]));
        assert!(!stable_top_level_identifier_seen(&visited, 42, &[1]));
        mark_stable_top_level_identifier(&mut visited, 99, &[1]);
        assert!(!visited.contains(&99));
    }

    #[test]
    fn repeated_identifier_folders_have_distinct_recovery_units() {
        let first = RecoveryUnit::Normal {
            folder: FolderAddress::root().child(3).expect("first child"),
            folder_id: 42,
            message_index: 1,
        };
        let second = RecoveryUnit::Normal {
            folder: FolderAddress::root().child(4).expect("second child"),
            folder_id: 42,
            message_index: 1,
        };
        assert_ne!(first, second);
        assert_eq!(HashSet::from([first, second]).len(), 2);
    }

    #[test]
    fn sink_and_limit_failures_are_fatal_while_native_item_errors_are_recorded() {
        let mut catalog = RawCatalog::default();
        assert!(matches!(
            record_item_issue(
                &mut catalog,
                None,
                "test",
                PffError::Sink {
                    operation: "sink",
                    detail: "rejected".to_owned()
                }
            ),
            Err(PffError::Sink { .. })
        ));
        assert!(matches!(
            record_item_issue(
                &mut catalog,
                None,
                "test",
                PffError::LimitExceeded {
                    field: "count",
                    value: 2,
                    limit: 1
                }
            ),
            Err(PffError::LimitExceeded { .. })
        ));
        record_attachment_issue(
            &mut catalog,
            Some(6),
            "stream attachments",
            PffError::LimitExceeded {
                field: "attachment count",
                value: 2,
                limit: 1,
            },
        )
        .expect("attachment limit damage remains candidate-local");
        record_item_issue(
            &mut catalog,
            Some(7),
            "test",
            PffError::Native {
                operation: "read",
                detail: "damaged".to_owned(),
            },
        )
        .expect("native item damage remains recoverable");
        assert_eq!(catalog.issues.len(), 2);
        assert_eq!(catalog.issues[0].node_id, Some(6));
        assert_eq!(catalog.issues[1].node_id, Some(7));
    }

    #[test]
    fn damaged_optional_metadata_defaults_and_marks_the_candidate_partial() {
        let mut catalog = RawCatalog::default();
        let value: Option<String> = recover_item_value(
            &mut catalog,
            Some(9),
            "get subject",
            Err(PffError::Native {
                operation: "read subject",
                detail: "damaged".to_owned(),
            }),
        )
        .expect("native metadata damage is recoverable");
        assert_eq!(value, None);
        assert_eq!(catalog.issues.len(), 1);
        assert_eq!(catalog.issues[0].node_id, Some(9));
    }

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
