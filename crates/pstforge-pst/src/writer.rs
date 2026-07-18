//! Creation of compact Unicode version 23 PST stores.

use byteorder::{LittleEndian, WriteBytesExt};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read, Seek, SeekFrom, Write},
    ops::Range,
    os::fd::AsRawFd,
    os::unix::fs::{MetadataExt, PermissionsExt},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

use crate::{
    PstFile, UnicodePstFile,
    block_sig::compute_sig,
    ltp::{
        heap::{
            HeapFillLevel, HeapId, HeapNodeBitmapHeader, HeapNodeHeader, HeapNodePageHeader,
            HeapNodeType,
        },
        prop_type::PropertyType,
        read_write::{HeapNodePageReadWrite, TableContextInfoReadWrite},
        table_context::{
            LTP_ROW_ID_PROP_ID, LTP_ROW_VERSION_PROP_ID, TableColumnDescriptor, TableContextInfo,
        },
        tree::HeapTreeHeader,
    },
    ndb::{
        block::{
            DataTreeBlockHeader, UnicodeBlockTrailer, UnicodeDataBlock, UnicodeDataTreeBlock,
            UnicodeDataTreeEntry, UnicodeIntermediateSubNodeTreeBlock,
            UnicodeIntermediateSubNodeTreeEntry, UnicodeLeafSubNodeTreeBlock,
            UnicodeLeafSubNodeTreeEntry, UnicodeSubNodeTreeBlockHeader, block_size,
        },
        block_id::{UnicodeBlockId, UnicodePageId},
        block_ref::{UnicodeBlockRef, UnicodePageRef},
        byte_index::UnicodeByteIndex,
        header::{NdbCryptMethod, UnicodeHeader},
        node_id::{
            NID_MESSAGE_STORE, NID_NAME_TO_ID_MAP, NID_ROOT_FOLDER, NID_SEARCH_ACTIVITY_LIST,
            NID_SEARCH_MANAGEMENT_QUEUE, NodeId, NodeIdType,
        },
        page::{
            BTreeEntry, BTreePageEntry, DENSITY_LIST_FILE_OFFSET, DensityListPageEntry,
            NodeBTreeEntry, PageType, UnicodeBTreeEntryPage, UnicodeBTreePageEntry,
            UnicodeBlockBTreeEntry, UnicodeBlockBTreePage, UnicodeDensityListPage, UnicodeMapPage,
            UnicodeNodeBTreeEntry, UnicodeNodeBTreePage, UnicodePageTrailer,
        },
        read_write::{
            BTreePageEntryReadWrite, BlockReadWrite, DensityListPageReadWrite, HeaderReadWrite,
            IntermediateTreeBlockReadWrite, MapPageReadWrite, PageTrailerReadWrite,
            UnicodeBTreePageReadWrite,
        },
        root::{AmapStatus, UnicodeRoot},
    },
};

const FIRST_AMAP: u64 = 0x4400;
const INITIAL_FILE_EOF: u64 = FIRST_AMAP + AMAP_DATA_SIZE;
const FIRST_PMAP: u64 = 0x4600;
const FIRST_DATA: u64 = 0x4800;
const SLOTS_PER_AMAP: u64 = 496 * 8;
const SLOT_SIZE: u64 = 64;
const PAGE_SIZE: u64 = 512;
const AMAP_DATA_SIZE: u64 = SLOTS_PER_AMAP * SLOT_SIZE;
const FMAP_FIRST_AMAP: u64 = 128;
const FMAP_AMAP_COUNT: u64 = 496;
const FPMAP_FIRST_AMAP: u64 = 128 * 64;
const FPMAP_AMAP_COUNT: u64 = 496 * 64;
const IPM_FOLDER_INDEX: u32 = 0x401;
const SEARCH_ROOT_INDEX: u32 = 0x402;
const DELETED_FOLDER_INDEX: u32 = 0x403;
const MAIL_FOLDER_INDEX: u32 = 0x404;
const MSGFLAG_ASSOCIATED: i32 = 0x0000_0040;
const SPAM_SEARCH_INDEX: u32 = 0x111;
const MESSAGE_INDEX: u32 = 0x10001;
const NID_HIERARCHY_TABLE_TEMPLATE: u32 = 0x60D;
const NID_CONTENTS_TABLE_TEMPLATE: u32 = 0x60E;
const NID_ASSOC_CONTENTS_TABLE_TEMPLATE: u32 = 0x60F;
const NID_SEARCH_CONTENTS_TABLE_TEMPLATE: u32 = 0x610;
const NID_RECEIVE_FOLDER_TABLE: u32 = 0x62B;
const NID_OUTGOING_QUEUE_TABLE: u32 = 0x64C;
const NID_ATTACHMENT_TABLE_TEMPLATE: u32 = 0x671;
const NID_RECIPIENT_TABLE_TEMPLATE: u32 = 0x692;
const NID_CONTENTS_INDEX_TEMPLATE: u32 = 0x6B6;
const NID_SEARCH_INDEX_TEMPLATE: u32 = 0x6D7;
const NID_ATTACHMENT_INDEX_TEMPLATE: u32 = 0x6F8;
const NID_HIERARCHY_MAP: u32 = 0xC01;
const NID_SEARCH_FOLDER_TEMPLATE: u32 = 0xEC1;
const INITIAL_NID_COUNTERS: [u32; 32] = [
    0x400, 0x400, 0x400, 0x4000, 0x10000, 0x400, 0x400, 0x400, 0x8000, 0x400, 0x400, 0x400, 0x400,
    0x400, 0x400, 0x400, 0x400, 0x400, 0x400, 0x400, 0x400, 0x400, 0x400, 0x400, 0x400, 0x400,
    0x400, 0x400, 0x400, 0x400, 0x400, 0x400,
];
const MAX_EMBEDDED_MESSAGE_DEPTH: usize = 256;
const WRITER_STACK_BYTES: usize = 32 * 1024 * 1024;

/// Inputs for the first observable writer milestone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinimalStore {
    pub store_name: String,
    pub folder_name: String,
    pub subject: String,
    pub body: String,
    pub sender_name: String,
    pub sender_email: String,
    pub recipient: String,
    pub record_key: [u8; 16],
}

/// A recipient serialized into the message recipient table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecipientSpec {
    pub kind: RecipientKind,
    pub display_name: String,
    pub email_address: String,
}

/// MAPI recipient roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum RecipientKind {
    To = 1,
    Cc = 2,
    Bcc = 3,
}

/// The authoritative representation selected by MAPI best-body retrieval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum NativeBody {
    PlainText = 1,
    Rtf = 2,
    Html = 3,
}

/// Attachment content supported by the 0.2.1 writer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentContent {
    Binary(Vec<u8>),
    Spooled(FileBlobSpec),
    Embedded(Box<MessageSpec>),
}

/// Immutable private-spool payload streamed into a PST data tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileBlobSpec {
    pub path: PathBuf,
    pub offset: u64,
    pub byte_len: u64,
    pub sha256: [u8; 32],
}

/// A raw encoded MAPI property streamed from an immutable private spool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpooledPropertySpec {
    pub id: u16,
    pub property_type: u16,
    pub blob: FileBlobSpec,
}

/// A by-value file or embedded-message attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentSpec {
    pub filename: String,
    pub mime_type: Option<String>,
    pub content_id: Option<String>,
    pub content_location: Option<String>,
    pub rendering_position: Option<i32>,
    pub flags: i32,
    pub raw_properties: Vec<RawProperty>,
    pub content: AttachmentContent,
}

/// Safely serializable MAPI property values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawPropertyValue {
    Integer16(i16),
    Integer32(i32),
    Integer64(i64),
    Floating32(u32),
    Floating64(u64),
    Currency(i64),
    FloatingTime(u64),
    ErrorCode(u32),
    Boolean(bool),
    Time(i64),
    Guid([u8; 16]),
    Unicode(String),
    Binary(Vec<u8>),
    MultipleInteger16(Vec<i16>),
    MultipleInteger32(Vec<i32>),
    MultipleInteger64(Vec<i64>),
    MultipleGuid(Vec<[u8; 16]>),
}

/// A raw property retained when its type has an unambiguous PST encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawProperty {
    pub id: u16,
    pub value: RawPropertyValue,
}

/// Source property that cannot yet be represented without loss.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedProperty {
    pub id: u16,
    pub property_type: u16,
    pub byte_len: u64,
}

/// Unsupported property associated with its deterministic message path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedPropertyRecord {
    /// Empty for the top-level message; attachment indexes locate embedded messages.
    pub message_path: Vec<u32>,
    pub property: UnsupportedProperty,
}

/// Explicit accounting returned for properties intentionally not serialized.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FidelityWriteReport {
    pub unsupported_properties: Vec<UnsupportedPropertyRecord>,
}

/// Well-known named-property GUID sets supported by the writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NamedPropertySet {
    Mapi,
    PublicStrings,
    Guid([u8; 16]),
}

/// A numeric dispatch identifier or Unicode named-property string.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NamedPropertyName {
    Numeric(u32),
    String(String),
}

/// A named MAPI property whose 0x8000-range ID is assigned per store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedProperty {
    pub set: NamedPropertySet,
    pub name: NamedPropertyName,
    pub value: RawPropertyValue,
}

/// Canonical mail input for the 0.2.1 fidelity writer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageSpec {
    pub message_class: String,
    pub message_flags: i32,
    pub internet_codepage: i32,
    pub subject: String,
    pub sender_name: String,
    pub sender_email: String,
    pub recipients: Vec<RecipientSpec>,
    pub sent_filetime: i64,
    pub received_filetime: i64,
    pub creation_filetime: i64,
    pub modification_filetime: i64,
    pub body_text: Option<String>,
    pub body_html: Option<Vec<u8>>,
    pub body_rtf: Option<Vec<u8>>,
    pub native_body: Option<NativeBody>,
    pub rtf_in_sync: bool,
    pub internet_headers: Option<String>,
    pub attachments: Vec<AttachmentSpec>,
    pub named_properties: Vec<NamedProperty>,
    pub raw_properties: Vec<RawProperty>,
    pub spooled_properties: Vec<SpooledPropertySpec>,
    pub unsupported_properties: Vec<UnsupportedProperty>,
}

/// One deterministic folder and message used as the 0.2.1 writer boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FidelityStore {
    pub store_name: String,
    pub folder_name: String,
    pub record_key: [u8; 16],
    pub message: MessageSpec,
}

/// One source folder and its top-level mail in a split output part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailFolderSpec {
    /// Non-empty path below the declared PST subtree.
    pub path: Vec<String>,
    pub location: MailFolderLocation,
    pub role: MailFolderRole,
    /// Source PR_CONTAINER_CLASS, or `IPF.Note` when the property was absent.
    pub container_class: String,
    pub messages: Vec<MessageSpec>,
    pub associated_messages: Vec<MessageSpec>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum MailFolderLocation {
    StoreRoot,
    #[default]
    IpmSubtree,
}

/// A source folder's structural role, independent of its display name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum MailFolderRole {
    #[default]
    Ordinary,
    DeletedItems,
}

/// Deterministic multi-folder input for size-limited output parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailStoreSpec {
    pub store_name: String,
    pub record_key: [u8; 16],
    pub folders: Vec<MailFolderSpec>,
}

struct StoreInput<'a> {
    store_name: &'a str,
    folder_name: &'a str,
    record_key: [u8; 16],
    message: &'a MessageSpec,
    associated: bool,
}

impl From<&MinimalStore> for FidelityStore {
    fn from(spec: &MinimalStore) -> Self {
        const FIXED_FILETIME: i64 = 133_801_632_000_000_000;
        Self {
            store_name: spec.store_name.clone(),
            folder_name: spec.folder_name.clone(),
            record_key: spec.record_key,
            message: MessageSpec {
                message_class: "IPM.Note".to_owned(),
                message_flags: 1,
                internet_codepage: 65001,
                subject: spec.subject.clone(),
                sender_name: spec.sender_name.clone(),
                sender_email: spec.sender_email.clone(),
                recipients: vec![RecipientSpec {
                    kind: RecipientKind::To,
                    display_name: spec.recipient.clone(),
                    email_address: spec.recipient.clone(),
                }],
                sent_filetime: FIXED_FILETIME,
                received_filetime: FIXED_FILETIME,
                creation_filetime: FIXED_FILETIME,
                modification_filetime: FIXED_FILETIME,
                body_text: Some(spec.body.clone()),
                body_html: None,
                body_rtf: None,
                native_body: Some(NativeBody::PlainText),
                rtf_in_sync: false,
                internet_headers: None,
                attachments: Vec::new(),
                named_properties: Vec::new(),
                raw_properties: Vec::new(),
                spooled_properties: Vec::new(),
                unsupported_properties: Vec::new(),
            },
        }
    }
}

impl Default for FidelityStore {
    fn default() -> Self {
        let recipient = |kind, display_name: &str, email_address: &str| RecipientSpec {
            kind,
            display_name: display_name.to_owned(),
            email_address: email_address.to_owned(),
        };
        let embedded = MessageSpec {
            message_class: "IPM.Note".to_owned(),
            message_flags: 1,
            internet_codepage: 65001,
            subject: "Embedded message checkpoint".to_owned(),
            sender_name: "Embedded Sender".to_owned(),
            sender_email: "embedded-sender@example.com".to_owned(),
            recipients: vec![recipient(
                RecipientKind::To,
                "Embedded Recipient",
                "embedded-recipient@example.com",
            )],
            sent_filetime: 133_801_632_100_000_000,
            received_filetime: 133_801_632_200_000_000,
            creation_filetime: 133_801_632_100_000_000,
            modification_filetime: 133_801_632_200_000_000,
            body_text: Some("Embedded plain-text body.".to_owned()),
            body_html: None,
            body_rtf: None,
            native_body: Some(NativeBody::PlainText),
            rtf_in_sync: false,
            internet_headers: None,
            attachments: Vec::new(),
            named_properties: vec![NamedProperty {
                set: NamedPropertySet::Guid(*b"PSTForgeEmbedSet"),
                name: NamedPropertyName::String("EmbeddedCheckpoint".to_owned()),
                value: RawPropertyValue::Boolean(true),
            }],
            raw_properties: vec![RawProperty {
                id: 0x10F7,
                value: RawPropertyValue::MultipleInteger32(vec![7, 11]),
            }],
            spooled_properties: Vec::new(),
            unsupported_properties: Vec::new(),
        };
        Self {
            store_name: "PSTForge 0.2.1 Mail Fidelity".to_owned(),
            folder_name: "Fidelity Mail".to_owned(),
            record_key: *b"PSTFORGE-0.2.1!!",
            message: MessageSpec {
                message_class: "IPM.Note".to_owned(),
                message_flags: 1,
                internet_codepage: 65001,
                subject: "Unicode fidelity: \u{20ac} \u{4e16}\u{754c}".to_owned(),
                sender_name: "PSTForge Sender".to_owned(),
                sender_email: "sender@example.com".to_owned(),
                recipients: vec![
                    recipient(RecipientKind::To, "Primary Recipient", "to@example.com"),
                    recipient(RecipientKind::Cc, "Copy Recipient", "cc@example.com"),
                    recipient(RecipientKind::Bcc, "Blind Recipient", "bcc@example.com"),
                ],
                sent_filetime: 133_801_632_300_000_000,
                received_filetime: 133_801_632_400_000_000,
                creation_filetime: 133_801_632_300_000_000,
                modification_filetime: 133_801_632_400_000_000,
                body_text: Some("Plain-text body checkpoint.".to_owned()),
                body_html: Some(
                    "<html><body><p><strong>HTML body checkpoint: € 世界.</strong></p></body></html>"
                        .as_bytes()
                        .to_vec(),
                ),
                body_rtf: Some(br"{\rtf1\ansi\b RTF body checkpoint.\b0}".to_vec()),
                native_body: Some(NativeBody::Html),
                rtf_in_sync: false,
                internet_headers: Some(
                    "Message-ID: <pstforge-fidelity@example.com>\r\nX-PSTForge: 0.2.1\r\n"
                        .to_owned(),
                ),
                attachments: vec![
                    AttachmentSpec {
                        filename: "checkpoint.txt".to_owned(),
                        mime_type: Some("text/plain".to_owned()),
                        content_id: Some("checkpoint@pstforge".to_owned()),
                        content_location: Some("checkpoint.txt".to_owned()),
                        rendering_position: Some(0),
                        flags: 4,
                        raw_properties: Vec::new(),
                        content: AttachmentContent::Binary(
                            (0..16 * 1024)
                                .map(|index| b'A' + (index % 26) as u8)
                                .collect(),
                        ),
                    },
                    AttachmentSpec {
                        filename: "embedded.msg".to_owned(),
                        mime_type: Some("application/vnd.ms-outlook".to_owned()),
                        content_id: None,
                        content_location: None,
                        rendering_position: None,
                        flags: 0,
                        raw_properties: Vec::new(),
                        content: AttachmentContent::Embedded(Box::new(embedded)),
                    },
                ],
                named_properties: vec![
                    NamedProperty {
                        set: NamedPropertySet::Mapi,
                        name: NamedPropertyName::Numeric(0x8005),
                        value: RawPropertyValue::Unicode("named property checkpoint".to_owned()),
                    },
                    NamedProperty {
                        set: NamedPropertySet::Guid(*b"PSTForgeNamedSet"),
                        name: NamedPropertyName::String("CustomCheckpoint".to_owned()),
                        value: RawPropertyValue::Integer32(21),
                    },
                ],
                raw_properties: vec![
                    RawProperty {
                        id: 0x10F4,
                        value: RawPropertyValue::Unicode("raw property checkpoint".to_owned()),
                    },
                    RawProperty {
                        id: 0x10F5,
                        value: RawPropertyValue::Guid(*b"PSTForgeRawGuid!"),
                    },
                    RawProperty {
                        id: 0x10F6,
                        value: RawPropertyValue::MultipleGuid(vec![
                            *b"PSTForgeGuidOne!",
                            *b"PSTForgeGuidTwo!",
                        ]),
                    },
                ],
                spooled_properties: Vec::new(),
                unsupported_properties: Vec::new(),
            },
        }
    }
}

impl Default for MinimalStore {
    fn default() -> Self {
        Self {
            store_name: "PSTForge".to_owned(),
            folder_name: "Recovered Mail".to_owned(),
            subject: "PSTForge writer checkpoint".to_owned(),
            body: "This message verifies Unicode PST creation.".to_owned(),
            sender_name: "PSTForge Sender".to_owned(),
            sender_email: "sender@example.com".to_owned(),
            recipient: "recipient@example.com".to_owned(),
            record_key: [
                0x50, 0x53, 0x54, 0x46, 0x4f, 0x52, 0x47, 0x45, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x01,
            ],
        }
    }
}

#[derive(Debug, Error)]
pub enum WriterError {
    #[error("PST creation was interrupted")]
    Interrupted,
    #[error("output already exists: {0}")]
    OutputExists(PathBuf),
    #[error("output was published at {path}, but its directory sync failed: {source}")]
    PublishedDurability {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("output was published, but the requested path no longer names it: {0}")]
    PublishedDestinationChanged(PathBuf),
    #[error("independent PST validator {tool} failed: {source}")]
    IndependentValidatorIo {
        tool: &'static str,
        #[source]
        source: io::Error,
    },
    #[error(
        "independent PST validator {tool} rejected the completed store; unpublished evidence retained at {evidence}"
    )]
    IndependentValidation {
        tool: &'static str,
        evidence: PathBuf,
    },
    #[error("writer value is too large for the PST structure: {0}")]
    ValueTooLarge(&'static str),
    #[error("invalid PST structure: {0}")]
    InvalidStructure(String),
    #[error("PST input cannot be represented: {0}")]
    InputRejected(String),
    #[error("completed PST validation failed: {0}")]
    CompletedValidation(String),
    #[error("PST writer execution thread terminated unexpectedly")]
    ExecutionTerminated,
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

fn with_writer_stack<T, F>(operation: F) -> Result<T, WriterError>
where
    T: Send,
    F: FnOnce() -> Result<T, WriterError> + Send,
{
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("pstforge-writer".to_owned())
            .stack_size(WRITER_STACK_BYTES)
            .spawn_scoped(scope, operation)
            .map_err(WriterError::Io)?
            .join()
            .map_err(|_| WriterError::ExecutionTerminated)?
    })
}

#[derive(Clone, PartialEq, Eq)]
enum PropertyValue {
    Integer16(i16),
    Integer32(i32),
    Integer64(i64),
    Floating32(u32),
    Floating64(u64),
    Currency(i64),
    FloatingTime(u64),
    ErrorCode(u32),
    Boolean(bool),
    Time(i64),
    Guid([u8; 16]),
    Unicode(String),
    Binary(Vec<u8>),
    MultipleInteger16(Vec<i16>),
    MultipleInteger32(Vec<i32>),
    MultipleInteger64(Vec<i64>),
    MultipleGuid(Vec<[u8; 16]>),
    Object(NodeId, u32),
    External(PropertyType, NodeId),
}

impl PropertyValue {
    fn property_type(&self) -> PropertyType {
        match self {
            Self::Integer16(_) => PropertyType::Integer16,
            Self::Integer32(_) => PropertyType::Integer32,
            Self::Integer64(_) => PropertyType::Integer64,
            Self::Floating32(_) => PropertyType::Floating32,
            Self::Floating64(_) => PropertyType::Floating64,
            Self::Currency(_) => PropertyType::Currency,
            Self::FloatingTime(_) => PropertyType::FloatingTime,
            Self::ErrorCode(_) => PropertyType::ErrorCode,
            Self::Boolean(_) => PropertyType::Boolean,
            Self::Time(_) => PropertyType::Time,
            Self::Guid(_) => PropertyType::Guid,
            Self::Unicode(_) => PropertyType::Unicode,
            Self::Binary(_) => PropertyType::Binary,
            Self::MultipleInteger16(_) => PropertyType::MultipleInteger16,
            Self::MultipleInteger32(_) => PropertyType::MultipleInteger32,
            Self::MultipleInteger64(_) => PropertyType::MultipleInteger64,
            Self::MultipleGuid(_) => PropertyType::MultipleGuid,
            Self::Object(_, _) => PropertyType::Object,
            Self::External(kind, _) => *kind,
        }
    }

    fn inline_value(&self) -> Option<u32> {
        match self {
            Self::Integer16(value) => Some(u32::from(u16::from_le_bytes(value.to_le_bytes()))),
            Self::Integer32(value) => Some(u32::from_le_bytes(value.to_le_bytes())),
            Self::Floating32(value) | Self::ErrorCode(value) => Some(*value),
            Self::Boolean(value) => Some(u32::from(*value)),
            Self::External(_, node) => Some(u32::from(*node)),
            _ => None,
        }
    }

    fn variable_bytes(&self) -> io::Result<Option<Vec<u8>>> {
        let bytes = match self {
            Self::Integer64(value) | Self::Currency(value) | Self::Time(value) => {
                value.to_le_bytes().to_vec()
            }
            Self::Floating64(value) | Self::FloatingTime(value) => value.to_le_bytes().to_vec(),
            Self::Guid(value) => value.to_vec(),
            Self::Unicode(value) => unicode_bytes(value)?,
            Self::Binary(value) => value.clone(),
            Self::MultipleInteger16(values) => values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
            Self::MultipleInteger32(values) => values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
            Self::MultipleInteger64(values) => values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
            Self::MultipleGuid(values) => {
                let capacity = values.len().checked_mul(16).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "GUID values are too large")
                })?;
                let mut bytes = Vec::with_capacity(capacity);
                bytes.extend(values.iter().flatten().copied());
                bytes
            }
            Self::Object(node, size) => {
                let mut bytes = Vec::with_capacity(8);
                bytes.extend_from_slice(&u32::from(*node).to_le_bytes());
                bytes.extend_from_slice(&size.to_le_bytes());
                bytes
            }
            Self::External(_, _) => return Ok(None),
            Self::Integer16(_)
            | Self::Integer32(_)
            | Self::Floating32(_)
            | Self::ErrorCode(_)
            | Self::Boolean(_) => return Ok(None),
        };
        Ok(Some(bytes))
    }
}

struct BlockSpec {
    id: UnicodeBlockId,
    payload: BlockPayload,
    ref_count: u16,
}

enum BlockPayload {
    Data(Vec<u8>),
    Subnode(Vec<UnicodeLeafSubNodeTreeEntry>),
    IntermediateSubnode {
        level: u8,
        entries: Vec<UnicodeIntermediateSubNodeTreeEntry>,
    },
    DataTree {
        level: u8,
        total_size: u32,
        entries: Vec<UnicodeDataTreeEntry>,
    },
}

impl BlockPayload {
    fn logical_size(&self) -> usize {
        match self {
            Self::Data(data) => data.len(),
            Self::Subnode(entries) => 8_usize.saturating_add(entries.len().saturating_mul(24)),
            Self::IntermediateSubnode { entries, .. } => {
                8_usize.saturating_add(entries.len().saturating_mul(16))
            }
            Self::DataTree { entries, .. } => {
                8_usize.saturating_add(entries.len().saturating_mul(8))
            }
        }
    }
}

struct WrittenBlock {
    id: UnicodeBlockId,
    offset: u64,
    size: u16,
    ref_count: u16,
}

struct BlockStream<'a> {
    file: &'a mut std::fs::File,
    cursor: &'a mut u64,
    written: &'a mut Vec<WrittenBlock>,
    interrupted: &'a AtomicBool,
}

impl BlockStream<'_> {
    fn emit(&mut self, block: BlockSpec) -> Result<(), WriterError> {
        self.written.extend(write_blocks(
            self.file,
            &[block],
            self.cursor,
            self.interrupted,
        )?);
        Ok(())
    }
}

struct TableRowSpec {
    id: NodeId,
    values: Vec<(u16, PropertyValue)>,
}

struct ExternalTableBuild {
    data_block: UnicodeBlockId,
    subnode_block: UnicodeBlockId,
    blocks: Vec<BlockSpec>,
}

const MAX_DATA_BLOCK_PAYLOAD: usize = 8176;
const MAX_HEAP_ALLOCATION: usize = 3580;
const MAX_DATA_TREE_ENTRIES: usize = 1021;
const SUBNODE_LEAF_CAPACITY: usize = (MAX_DATA_BLOCK_PAYLOAD - 8) / 24;
const SUBNODE_INTERMEDIATE_CAPACITY: usize = (MAX_DATA_BLOCK_PAYLOAD - 8) / 16;
const MAX_SUBNODE_TREE_ENTRIES: usize = SUBNODE_LEAF_CAPACITY * SUBNODE_INTERMEDIATE_CAPACITY;
const MAX_FIDELITY_PROPERTY_BYTES: usize = 16 * 1024;
const MAX_FIDELITY_COLLECTION_ITEMS: usize = MAX_FIDELITY_PROPERTY_BYTES / 8;
const MAX_FIDELITY_CUSTOM_PROPERTY_BYTES: usize = 64 * 1024;
static NEVER_INTERRUPTED: AtomicBool = AtomicBool::new(false);

fn check_interrupted(interrupted: &AtomicBool) -> Result<(), WriterError> {
    if interrupted.load(Ordering::Relaxed) {
        Err(WriterError::Interrupted)
    } else {
        Ok(())
    }
}

fn externalize_large_properties(
    properties: &mut [(u16, PropertyValue)],
    next_block_index: &mut u64,
    next_value_node: &mut u32,
    blocks: &mut Vec<BlockSpec>,
    subnodes: &mut Vec<UnicodeLeafSubNodeTreeEntry>,
) -> Result<(), WriterError> {
    for (_, value) in properties {
        let kind = value.property_type();
        let Some(bytes) = value.variable_bytes()? else {
            continue;
        };
        if bytes.len() > MAX_FIDELITY_PROPERTY_BYTES {
            return Err(WriterError::ValueTooLarge(
                "0.2.1 canonical property payload (16 KiB)",
            ));
        }
        if bytes.len() <= 2048 || matches!(value, PropertyValue::Object(_, _)) {
            continue;
        }
        let node = node(NodeIdType::ListsTablesProperties, *next_value_node)?;
        *next_value_node = next_value_node
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("property subnode id"))?;
        let root = append_data_tree(&bytes, next_block_index, blocks)?;
        subnodes.push(UnicodeLeafSubNodeTreeEntry::new(node, root, None));
        *value = PropertyValue::External(kind, node);
    }
    Ok(())
}

fn append_data_tree(
    bytes: &[u8],
    next_block_index: &mut u64,
    blocks: &mut Vec<BlockSpec>,
) -> Result<UnicodeBlockId, WriterError> {
    append_data_tree_sized(bytes, MAX_DATA_BLOCK_PAYLOAD, next_block_index, blocks)
}

fn append_row_matrix_data_tree(
    bytes: &[u8],
    row_size: usize,
    next_block_index: &mut u64,
    blocks: &mut Vec<BlockSpec>,
) -> Result<UnicodeBlockId, WriterError> {
    if row_size == 0 || row_size > MAX_DATA_BLOCK_PAYLOAD {
        return Err(WriterError::ValueTooLarge("table row size"));
    }
    let chunk_size = (MAX_DATA_BLOCK_PAYLOAD / row_size) * row_size;
    let chunk_count = bytes.len().div_ceil(chunk_size);
    let padding_capacity = chunk_count
        .saturating_sub(1)
        .checked_mul(MAX_DATA_BLOCK_PAYLOAD - chunk_size)
        .ok_or(WriterError::ValueTooLarge("row matrix padding"))?;
    let mut framed = Vec::with_capacity(
        bytes
            .len()
            .checked_add(padding_capacity)
            .ok_or(WriterError::ValueTooLarge("row matrix padding"))?,
    );
    for (index, chunk) in bytes.chunks(chunk_size).enumerate() {
        framed.extend_from_slice(chunk);
        if index + 1 < chunk_count {
            framed.resize(
                framed
                    .len()
                    .checked_add(MAX_DATA_BLOCK_PAYLOAD - chunk.len())
                    .ok_or(WriterError::ValueTooLarge("row matrix padding"))?,
                0,
            );
        }
    }
    append_data_tree(&framed, next_block_index, blocks)
}

fn append_data_tree_sized(
    bytes: &[u8],
    chunk_size: usize,
    next_block_index: &mut u64,
    blocks: &mut Vec<BlockSpec>,
) -> Result<UnicodeBlockId, WriterError> {
    let mut leaves = Vec::with_capacity(bytes.len().div_ceil(chunk_size));
    for chunk in bytes.chunks(chunk_size) {
        let id = take_block_id(next_block_index, false)?;
        blocks.push(BlockSpec {
            id,
            payload: BlockPayload::Data(chunk.to_vec()),
            ref_count: 2,
        });
        leaves.push((id, chunk.len()));
    }
    if leaves.len() == 1 {
        return Ok(leaves[0].0);
    }

    let mut xblocks = Vec::with_capacity(leaves.len().div_ceil(MAX_DATA_TREE_ENTRIES));
    for group in leaves.chunks(MAX_DATA_TREE_ENTRIES) {
        let id = take_block_id(next_block_index, true)?;
        let total_size = group.iter().try_fold(0_u32, |total, (_, size)| {
            total.checked_add(u32::try_from(*size).ok()?)
        });
        let total_size = total_size.ok_or(WriterError::ValueTooLarge("data-tree size"))?;
        blocks.push(BlockSpec {
            id,
            payload: BlockPayload::DataTree {
                level: 1,
                total_size,
                entries: group
                    .iter()
                    .map(|(block, _)| UnicodeDataTreeEntry::from(*block))
                    .collect(),
            },
            ref_count: 2,
        });
        xblocks.push((id, total_size));
    }
    if xblocks.len() == 1 {
        return Ok(xblocks[0].0);
    }
    if xblocks.len() > MAX_DATA_TREE_ENTRIES {
        return Err(WriterError::ValueTooLarge("XXBLOCK entry count"));
    }
    let total_size = xblocks
        .iter()
        .try_fold(0_u32, |total, (_, size)| total.checked_add(*size))
        .ok_or(WriterError::ValueTooLarge("XXBLOCK size"))?;
    let id = take_block_id(next_block_index, true)?;
    blocks.push(BlockSpec {
        id,
        payload: BlockPayload::DataTree {
            level: 2,
            total_size,
            entries: xblocks
                .iter()
                .map(|(block, _)| UnicodeDataTreeEntry::from(*block))
                .collect(),
        },
        ref_count: 2,
    });
    Ok(id)
}

fn append_data_tree_pages(
    pages: &[Vec<u8>],
    next_block_index: &mut u64,
    blocks: &mut Vec<BlockSpec>,
) -> Result<UnicodeBlockId, WriterError> {
    if pages.is_empty() {
        return Err(WriterError::InvalidStructure(
            "data tree must contain pages".to_owned(),
        ));
    }
    let mut leaves = Vec::with_capacity(pages.len());
    for page in pages {
        if page.len() > MAX_DATA_BLOCK_PAYLOAD {
            return Err(WriterError::ValueTooLarge("heap data page"));
        }
        let id = take_block_id(next_block_index, false)?;
        blocks.push(BlockSpec {
            id,
            payload: BlockPayload::Data(page.clone()),
            ref_count: 2,
        });
        leaves.push((id, page.len()));
    }
    if leaves.len() == 1 {
        return Ok(leaves[0].0);
    }
    if leaves.len() > MAX_DATA_TREE_ENTRIES {
        return Err(WriterError::ValueTooLarge("heap data-tree page count"));
    }
    let total_size = leaves.iter().try_fold(0_u32, |total, (_, size)| {
        total.checked_add(u32::try_from(*size).ok()?)
    });
    let total_size = total_size.ok_or(WriterError::ValueTooLarge("heap data-tree size"))?;
    let id = take_block_id(next_block_index, true)?;
    blocks.push(BlockSpec {
        id,
        payload: BlockPayload::DataTree {
            level: 1,
            total_size,
            entries: leaves
                .iter()
                .map(|(block, _)| UnicodeDataTreeEntry::from(*block))
                .collect(),
        },
        ref_count: 2,
    });
    Ok(id)
}

fn append_spooled_data_tree(
    blob: &FileBlobSpec,
    next_block_index: &mut u64,
    stream: &mut BlockStream<'_>,
) -> Result<(UnicodeBlockId, usize), WriterError> {
    use std::os::unix::fs::OpenOptionsExt;

    let nofollow = i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())
        .map_err(|_| WriterError::ValueTooLarge("open flags"))?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nofollow)
        .open(&blob.path)?;
    let metadata = file.metadata()?;
    let end = blob
        .offset
        .checked_add(blob.byte_len)
        .ok_or(WriterError::ValueTooLarge("spooled attachment range"))?;
    if !metadata.file_type().is_file() || metadata.len() < end {
        return Err(WriterError::InvalidStructure(
            "spooled attachment identity mismatch".to_owned(),
        ));
    }
    file.seek(std::io::SeekFrom::Start(blob.offset))?;

    let mut remaining = blob.byte_len;
    let mut leaves = Vec::with_capacity(MAX_DATA_TREE_ENTRIES);
    let mut xblocks = Vec::new();
    let mut logical_size = 0_usize;
    let mut hasher = Sha256::new();
    while remaining > 0 {
        let length = usize::try_from(remaining.min(MAX_DATA_BLOCK_PAYLOAD as u64))
            .map_err(|_| WriterError::ValueTooLarge("spooled attachment chunk"))?;
        let mut chunk = vec![0_u8; length];
        file.read_exact(&mut chunk)?;
        hasher.update(&chunk);
        let id = take_block_id(next_block_index, false)?;
        let block = BlockSpec {
            id,
            payload: BlockPayload::Data(chunk),
            ref_count: 2,
        };
        logical_size = logical_size
            .checked_add(block.payload.logical_size())
            .ok_or(WriterError::ValueTooLarge("spooled attachment size"))?;
        stream.emit(block)?;
        leaves.push((id, length));
        remaining -= u64::try_from(length)
            .map_err(|_| WriterError::ValueTooLarge("spooled attachment chunk"))?;
        if leaves.len() == MAX_DATA_TREE_ENTRIES && remaining > 0 {
            let total_size = leaves.iter().try_fold(0_u32, |total, (_, size)| {
                total.checked_add(u32::try_from(*size).ok()?)
            });
            let total_size = total_size.ok_or(WriterError::ValueTooLarge("spooled data tree"))?;
            let id = take_block_id(next_block_index, true)?;
            let block = BlockSpec {
                id,
                payload: BlockPayload::DataTree {
                    level: 1,
                    total_size,
                    entries: leaves
                        .iter()
                        .map(|(block, _)| UnicodeDataTreeEntry::from(*block))
                        .collect(),
                },
                ref_count: 2,
            };
            logical_size = logical_size
                .checked_add(block.payload.logical_size())
                .ok_or(WriterError::ValueTooLarge("spooled attachment size"))?;
            stream.emit(block)?;
            xblocks.push((id, total_size));
            leaves.clear();
        }
    }
    let actual_hash: [u8; 32] = hasher.finalize().into();
    if actual_hash != blob.sha256 {
        return Err(WriterError::InvalidStructure(
            "spooled attachment hash mismatch".to_owned(),
        ));
    }
    if leaves.is_empty() {
        let id = take_block_id(next_block_index, false)?;
        let block = BlockSpec {
            id,
            payload: BlockPayload::Data(Vec::new()),
            ref_count: 2,
        };
        logical_size = logical_size
            .checked_add(block.payload.logical_size())
            .ok_or(WriterError::ValueTooLarge("spooled attachment size"))?;
        stream.emit(block)?;
        return Ok((id, logical_size));
    }
    if xblocks.is_empty() && leaves.len() == 1 {
        return Ok((leaves[0].0, logical_size));
    }

    if !leaves.is_empty() {
        let total_size = leaves.iter().try_fold(0_u32, |total, (_, size)| {
            total.checked_add(u32::try_from(*size).ok()?)
        });
        let total_size = total_size.ok_or(WriterError::ValueTooLarge("spooled data tree"))?;
        let id = take_block_id(next_block_index, true)?;
        let block = BlockSpec {
            id,
            payload: BlockPayload::DataTree {
                level: 1,
                total_size,
                entries: leaves
                    .iter()
                    .map(|(block, _)| UnicodeDataTreeEntry::from(*block))
                    .collect(),
            },
            ref_count: 2,
        };
        logical_size = logical_size
            .checked_add(block.payload.logical_size())
            .ok_or(WriterError::ValueTooLarge("spooled attachment size"))?;
        stream.emit(block)?;
        xblocks.push((id, total_size));
    }
    if xblocks.len() == 1 {
        return Ok((xblocks[0].0, logical_size));
    }
    if xblocks.len() > MAX_DATA_TREE_ENTRIES {
        return Err(WriterError::ValueTooLarge("spooled XXBLOCK entry count"));
    }
    let total_size = xblocks
        .iter()
        .try_fold(0_u32, |total, (_, size)| total.checked_add(*size))
        .ok_or(WriterError::ValueTooLarge("spooled XXBLOCK size"))?;
    let id = take_block_id(next_block_index, true)?;
    let block = BlockSpec {
        id,
        payload: BlockPayload::DataTree {
            level: 2,
            total_size,
            entries: xblocks
                .iter()
                .map(|(block, _)| UnicodeDataTreeEntry::from(*block))
                .collect(),
        },
        ref_count: 2,
    };
    logical_size = logical_size
        .checked_add(block.payload.logical_size())
        .ok_or(WriterError::ValueTooLarge("spooled attachment size"))?;
    stream.emit(block)?;
    Ok((id, logical_size))
}

fn append_spooled_properties(
    specs: &[SpooledPropertySpec],
    properties: &mut Vec<(u16, PropertyValue)>,
    subnodes: &mut Vec<UnicodeLeafSubNodeTreeEntry>,
    next_block_index: &mut u64,
    next_value_node: &mut u32,
    stream: &mut BlockStream<'_>,
) -> Result<usize, WriterError> {
    let mut logical_size = 0_usize;
    for spec in specs {
        let property_type = PropertyType::try_from(spec.property_type).map_err(|_| {
            WriterError::InvalidStructure(format!(
                "unsupported streamed property type: 0x{:04X}",
                spec.property_type
            ))
        })?;
        let value_node = node(NodeIdType::ListsTablesProperties, *next_value_node)?;
        *next_value_node = next_value_node
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("streamed property value node"))?;
        let (root, size) = append_spooled_data_tree(&spec.blob, next_block_index, stream)?;
        logical_size = logical_size
            .checked_add(size)
            .ok_or(WriterError::ValueTooLarge("message size"))?;
        subnodes.push(UnicodeLeafSubNodeTreeEntry::new(value_node, root, None));
        properties.push((spec.id, PropertyValue::External(property_type, value_node)));
    }
    Ok(logical_size)
}

fn append_subnode_tree(
    mut entries: Vec<UnicodeLeafSubNodeTreeEntry>,
    next_block_index: &mut u64,
    blocks: &mut Vec<BlockSpec>,
) -> Result<UnicodeBlockId, WriterError> {
    if entries.is_empty() {
        return Err(WriterError::InvalidStructure(
            "subnode tree must contain entries".to_owned(),
        ));
    }
    if entries.len() > MAX_SUBNODE_TREE_ENTRIES {
        return Err(WriterError::ValueTooLarge("subnode tree entry count"));
    }
    entries.sort_by_key(|entry| u32::from(entry.node()));
    let mut roots = Vec::with_capacity(entries.len().div_ceil(SUBNODE_LEAF_CAPACITY));
    for group in entries.chunks(SUBNODE_LEAF_CAPACITY) {
        let id = take_block_id(next_block_index, true)?;
        blocks.push(BlockSpec {
            id,
            payload: BlockPayload::Subnode(group.to_vec()),
            ref_count: 2,
        });
        roots.push(UnicodeIntermediateSubNodeTreeEntry::new(
            group[0].node(),
            id,
        ));
    }

    let mut level = 1_u8;
    while roots.len() > 1 {
        let mut parents = Vec::with_capacity(roots.len().div_ceil(SUBNODE_INTERMEDIATE_CAPACITY));
        for group in roots.chunks(SUBNODE_INTERMEDIATE_CAPACITY) {
            let id = take_block_id(next_block_index, true)?;
            blocks.push(BlockSpec {
                id,
                payload: BlockPayload::IntermediateSubnode {
                    level,
                    entries: group.to_vec(),
                },
                ref_count: 2,
            });
            parents.push(UnicodeIntermediateSubNodeTreeEntry::new(
                group[0].node(),
                id,
            ));
        }
        roots = parents;
        level = level
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("subnode tree depth"))?;
    }
    Ok(roots[0].block())
}

fn take_block_id(next: &mut u64, internal: bool) -> Result<UnicodeBlockId, WriterError> {
    let index = *next;
    *next = next
        .checked_add(1)
        .ok_or(WriterError::ValueTooLarge("block id"))?;
    if internal {
        internal_bid(index)
    } else {
        leaf_bid(index)
    }
}

/// Create a new PST with a minimal folder and plain-text message.
pub fn create_minimal_store(
    path: impl AsRef<Path>,
    spec: &MinimalStore,
) -> Result<(), WriterError> {
    create_fidelity_store(path, &FidelityStore::from(spec)).map(|_| ())
}

struct MessageBlocks {
    property_context: Vec<u8>,
    recipient_table: Vec<u8>,
    attachment_table: Vec<u8>,
    subnodes: Vec<UnicodeLeafSubNodeTreeEntry>,
    dynamic_blocks: Vec<BlockSpec>,
    record_key: [u8; 16],
    message_size: i32,
    streamed_logical_size: usize,
}

struct BuiltTopMessage {
    property_block: UnicodeBlockId,
    recipient_block: UnicodeBlockId,
    attachment_block: UnicodeBlockId,
    subnode_block: UnicodeBlockId,
    shared_table_blocks: bool,
    message: MessageBlocks,
}

fn built_message_block_specs(built: BuiltTopMessage) -> Vec<BlockSpec> {
    let mut blocks = vec![
        BlockSpec {
            id: built.property_block,
            payload: BlockPayload::Data(built.message.property_context),
            ref_count: 2,
        },
        BlockSpec {
            id: built.recipient_block,
            payload: BlockPayload::Data(built.message.recipient_table),
            ref_count: if built.shared_table_blocks { 3 } else { 2 },
        },
        BlockSpec {
            id: built.attachment_block,
            payload: BlockPayload::Data(built.message.attachment_table),
            ref_count: if built.shared_table_blocks { 3 } else { 2 },
        },
        BlockSpec {
            id: built.subnode_block,
            payload: BlockPayload::Subnode(built.message.subnodes),
            ref_count: 2,
        },
    ];
    blocks.extend(built.message.dynamic_blocks);
    blocks.sort_by_key(|block| u64::from(block.id));
    blocks
}

fn collect_subnode_ids(blocks: &[BlockSpec], output: &mut Vec<NodeId>) {
    for block in blocks {
        if let BlockPayload::Subnode(entries) = &block.payload {
            output.extend(entries.iter().map(|entry| entry.node()));
        }
    }
}

fn message_requires_streaming(message: &MessageSpec) -> bool {
    !message.spooled_properties.is_empty()
        || message
            .attachments
            .iter()
            .any(|attachment| match &attachment.content {
                AttachmentContent::Spooled(_) => true,
                AttachmentContent::Embedded(message) => message_requires_streaming(message),
                AttachmentContent::Binary(_) => false,
            })
}

struct FolderPlan<'a> {
    path: Vec<String>,
    messages: Vec<&'a MessageSpec>,
    associated_messages: Vec<&'a MessageSpec>,
    location: MailFolderLocation,
    node: NodeId,
    parent: Option<usize>,
    children: Vec<usize>,
    container_class: String,
}

fn plan_folders<'a>(
    fallback_name: &'a str,
    fallback_messages: &[&'a MessageSpec],
    folders: Option<&'a [MailFolderSpec]>,
) -> Result<Vec<FolderPlan<'a>>, WriterError> {
    let mut paths =
        BTreeMap::<(MailFolderLocation, Vec<String>), (Vec<&MessageSpec>, Vec<&MessageSpec>)>::new(
        );
    let mut roles = BTreeMap::<(MailFolderLocation, Vec<String>), MailFolderRole>::new();
    let mut container_classes = BTreeMap::<(MailFolderLocation, Vec<String>), String>::new();
    if let Some(folders) = folders {
        let mut explicit_paths = BTreeSet::new();
        for folder in folders {
            if folder.path.is_empty() || folder.path.iter().any(String::is_empty) {
                return Err(WriterError::InvalidStructure(
                    "mail folder paths and components must be non-empty".to_owned(),
                ));
            }
            let key = (folder.location, folder.path.clone());
            if !explicit_paths.insert(key.clone()) {
                return Err(WriterError::InvalidStructure(
                    "duplicate mail folder path".to_owned(),
                ));
            }
            if folder.role == MailFolderRole::DeletedItems
                && (folder.location != MailFolderLocation::IpmSubtree || folder.path.len() != 1)
            {
                return Err(WriterError::InvalidStructure(
                    "the Deleted Items role must identify a top-level IPM folder".to_owned(),
                ));
            }
            validate_unicode("folder container class", &folder.container_class)?;
            if folder.container_class.is_empty() {
                return Err(WriterError::InvalidStructure(
                    "folder container class must be non-empty".to_owned(),
                ));
            }
            for depth in 1..=folder.path.len() {
                paths
                    .entry((folder.location, folder.path[..depth].to_vec()))
                    .or_default();
            }
            roles.insert(key.clone(), folder.role);
            container_classes.insert(key.clone(), folder.container_class.clone());
            let (messages, associated_messages) = paths.get_mut(&key).ok_or_else(|| {
                WriterError::InvalidStructure("mail folder path was not planned".to_owned())
            })?;
            messages.extend(folder.messages.iter());
            associated_messages.extend(folder.associated_messages.iter());
        }
        if roles
            .values()
            .filter(|role| **role == MailFolderRole::DeletedItems)
            .count()
            > 1
        {
            return Err(WriterError::InvalidStructure(
                "multiple folders claim the Deleted Items role".to_owned(),
            ));
        }
    } else {
        paths.insert(
            (
                MailFolderLocation::IpmSubtree,
                vec![fallback_name.to_owned()],
            ),
            (fallback_messages.to_vec(), Vec::new()),
        );
    }
    if paths
        .values()
        .all(|(messages, associated)| messages.is_empty() && associated.is_empty())
    {
        return Err(WriterError::InvalidStructure(
            "mail store must contain at least one message".to_owned(),
        ));
    }

    let path_indexes = paths
        .keys()
        .enumerate()
        .map(|(index, path)| (path.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut plans = paths
        .into_iter()
        .enumerate()
        .map(
            |(index, ((location, path), (messages, associated_messages)))| {
                let index =
                    u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("folder count"))?;
                let parent = (path.len() > 1)
                    .then(|| {
                        path_indexes
                            .get(&(location, path[..path.len() - 1].to_vec()))
                            .copied()
                    })
                    .flatten();
                let role = roles
                    .get(&(location, path.clone()))
                    .copied()
                    .unwrap_or_default();
                let container_class = container_classes
                    .get(&(location, path.clone()))
                    .cloned()
                    .unwrap_or_else(|| "IPF.Note".to_owned());
                let node = if role == MailFolderRole::DeletedItems {
                    node(NodeIdType::NormalFolder, DELETED_FOLDER_INDEX)?
                } else {
                    node(
                        NodeIdType::NormalFolder,
                        MAIL_FOLDER_INDEX
                            .checked_add(index)
                            .ok_or(WriterError::ValueTooLarge("folder node"))?,
                    )?
                };
                Ok(FolderPlan {
                    path,
                    messages,
                    associated_messages,
                    location,
                    node,
                    parent,
                    children: Vec::new(),
                    container_class,
                })
            },
        )
        .collect::<Result<Vec<_>, WriterError>>()?;
    for child in 0..plans.len() {
        if let Some(parent) = plans[child].parent {
            plans[parent].children.push(child);
        }
    }
    Ok(plans)
}

#[allow(clippy::too_many_arguments)]
fn build_message_blocks(
    message_spec: &MessageSpec,
    associated: bool,
    record_key: [u8; 16],
    named_identities: &[NamedIdentity],
    recipient_block: UnicodeBlockId,
    attachment_table_block: UnicodeBlockId,
    next_block_index: &mut u64,
    next_value_node: &mut u32,
    mut block_stream: Option<&mut BlockStream<'_>>,
) -> Result<MessageBlocks, WriterError> {
    let recipient_columns = recipient_columns()?;
    let attachment_columns = attachment_columns()?;
    let recipient_rows = message_spec
        .recipients
        .iter()
        .enumerate()
        .map(|(index, recipient)| recipient_table_row(index, recipient))
        .collect::<Result<Vec<_>, _>>()?;
    let recipient_table = table_context(&recipient_columns, &recipient_rows)?;
    let mut attachment_rows = Vec::new();
    let mut dynamic_blocks = Vec::new();
    let mut streamed_logical_size = 0_usize;
    let mut message_subnodes = vec![
        UnicodeLeafSubNodeTreeEntry::new(
            NodeId::from(NID_RECIPIENT_TABLE_TEMPLATE),
            recipient_block,
            None,
        ),
        UnicodeLeafSubNodeTreeEntry::new(
            NodeId::from(NID_ATTACHMENT_TABLE_TEMPLATE),
            attachment_table_block,
            None,
        ),
    ];
    for (index, attachment) in message_spec.attachments.iter().enumerate() {
        let attachment_index =
            u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("attachment count"))?;
        let attachment_node = node(
            NodeIdType::Attachment,
            0x2_0000_u32
                .checked_add(attachment_index)
                .ok_or(WriterError::ValueTooLarge("attachment node"))?,
        )?;
        let attachment_block = take_block_id(next_block_index, false)?;
        let mut attachment_local_subnodes = Vec::new();
        let mut streamed_size = None;
        let (method, data_property) = match &attachment.content {
            AttachmentContent::Binary(data) => (1, PropertyValue::Binary(data.clone())),
            AttachmentContent::Spooled(blob) => {
                let value_node = node(NodeIdType::ListsTablesProperties, *next_value_node)?;
                *next_value_node = next_value_node
                    .checked_add(1)
                    .ok_or(WriterError::ValueTooLarge("attachment value node"))?;
                let stream = block_stream.as_deref_mut().ok_or_else(|| {
                    WriterError::InvalidStructure(
                        "spooled attachment requires streaming output".to_owned(),
                    )
                })?;
                let (root, logical_size) =
                    append_spooled_data_tree(blob, next_block_index, stream)?;
                streamed_logical_size = streamed_logical_size
                    .checked_add(logical_size)
                    .ok_or(WriterError::ValueTooLarge("message size"))?;
                attachment_local_subnodes
                    .push(UnicodeLeafSubNodeTreeEntry::new(value_node, root, None));
                streamed_size = Some(blob.byte_len);
                (1, PropertyValue::External(PropertyType::Binary, value_node))
            }
            AttachmentContent::Embedded(embedded) => {
                let embedded_node = node(
                    NodeIdType::NormalMessage,
                    0x3_0000_u32
                        .checked_add(attachment_index)
                        .ok_or(WriterError::ValueTooLarge("embedded message node"))?,
                )?;
                let embedded_pc_block = take_block_id(next_block_index, false)?;
                let embedded_recipient_block = take_block_id(next_block_index, false)?;
                let embedded_attachment_block = take_block_id(next_block_index, false)?;
                let embedded_blocks = build_message_blocks(
                    embedded,
                    false,
                    embedded_message_record_key(record_key, attachment_index),
                    named_identities,
                    embedded_recipient_block,
                    embedded_attachment_block,
                    next_block_index,
                    next_value_node,
                    block_stream.as_deref_mut(),
                )?;
                streamed_logical_size = streamed_logical_size
                    .checked_add(embedded_blocks.streamed_logical_size)
                    .ok_or(WriterError::ValueTooLarge("message size"))?;
                let embedded_size = embedded_blocks.message_size;
                let embedded_object_size = u32::try_from(embedded_size)
                    .map_err(|_| WriterError::ValueTooLarge("embedded message"))?;
                let embedded_subnode_block = take_block_id(next_block_index, true)?;
                dynamic_blocks.extend([
                    BlockSpec {
                        id: embedded_pc_block,
                        payload: BlockPayload::Data(embedded_blocks.property_context),
                        ref_count: 2,
                    },
                    BlockSpec {
                        id: embedded_recipient_block,
                        payload: BlockPayload::Data(embedded_blocks.recipient_table),
                        ref_count: 2,
                    },
                    BlockSpec {
                        id: embedded_attachment_block,
                        payload: BlockPayload::Data(embedded_blocks.attachment_table),
                        ref_count: 2,
                    },
                    BlockSpec {
                        id: embedded_subnode_block,
                        payload: BlockPayload::Subnode(embedded_blocks.subnodes),
                        ref_count: 2,
                    },
                ]);
                dynamic_blocks.extend(embedded_blocks.dynamic_blocks);
                attachment_local_subnodes.push(UnicodeLeafSubNodeTreeEntry::new(
                    embedded_node,
                    embedded_pc_block,
                    Some(embedded_subnode_block),
                ));
                (
                    5,
                    PropertyValue::Object(embedded_node, embedded_object_size),
                )
            }
        };
        let attachment_number =
            i32::try_from(index).map_err(|_| WriterError::ValueTooLarge("attachment number"))?;
        let mut properties =
            attachment_properties(attachment, attachment_number, method, 0, data_property);
        let size = attachment_property_size_with_stream(&properties, streamed_size)?;
        set_attachment_size(&mut properties, size)?;
        attachment_rows.push(attachment_table_row(
            attachment_node,
            attachment,
            attachment_number,
            method,
            size,
        ));
        externalize_large_properties(
            &mut properties,
            next_block_index,
            next_value_node,
            &mut dynamic_blocks,
            &mut attachment_local_subnodes,
        )?;
        dynamic_blocks.push(BlockSpec {
            id: attachment_block,
            payload: BlockPayload::Data(property_context(&properties)?),
            ref_count: 2,
        });
        attachment_local_subnodes.sort_by_key(|entry| u32::from(entry.node()));
        let attachment_subnode = if attachment_local_subnodes.is_empty() {
            None
        } else {
            let block = take_block_id(next_block_index, true)?;
            dynamic_blocks.push(BlockSpec {
                id: block,
                payload: BlockPayload::Subnode(attachment_local_subnodes),
                ref_count: 2,
            });
            Some(block)
        };
        message_subnodes.push(UnicodeLeafSubNodeTreeEntry::new(
            attachment_node,
            attachment_block,
            attachment_subnode,
        ));
    }
    message_subnodes.sort_by_key(|entry| u32::from(entry.node()));
    let attachment_table = table_context(&attachment_columns, &attachment_rows)?;
    let mut top_properties =
        message_properties(message_spec, associated, named_identities, record_key, 0)?;
    if !message_spec.spooled_properties.is_empty() {
        let stream = block_stream.ok_or_else(|| {
            WriterError::InvalidStructure("spooled property requires streaming output".to_owned())
        })?;
        streamed_logical_size = streamed_logical_size
            .checked_add(append_spooled_properties(
                &message_spec.spooled_properties,
                &mut top_properties,
                &mut message_subnodes,
                next_block_index,
                next_value_node,
                stream,
            )?)
            .ok_or(WriterError::ValueTooLarge("message size"))?;
    }
    externalize_large_properties(
        &mut top_properties,
        next_block_index,
        next_value_node,
        &mut dynamic_blocks,
        &mut message_subnodes,
    )?;
    message_subnodes.sort_by_key(|entry| u32::from(entry.node()));
    let message_bytes = property_context(&top_properties)?
        .len()
        .checked_add(recipient_table.len())
        .and_then(|total| total.checked_add(attachment_table.len()))
        .and_then(|total| {
            dynamic_blocks.iter().try_fold(total, |sum, block| {
                sum.checked_add(block.payload.logical_size())
            })
        })
        .and_then(|total| total.checked_add(streamed_logical_size))
        .ok_or(WriterError::ValueTooLarge("message size"))?;
    let message_size =
        i32::try_from(message_bytes).map_err(|_| WriterError::ValueTooLarge("message size"))?;
    set_message_size(&mut top_properties, message_size)?;
    Ok(MessageBlocks {
        property_context: property_context(&top_properties)?,
        recipient_table,
        attachment_table,
        subnodes: message_subnodes,
        dynamic_blocks,
        record_key,
        message_size,
        streamed_logical_size,
    })
}

/// Create a deterministic Unicode PST containing one canonical mail message.
pub fn create_fidelity_store(
    path: impl AsRef<Path>,
    spec: &FidelityStore,
) -> Result<FidelityWriteReport, WriterError> {
    let path = path.as_ref();
    with_writer_stack(|| create_fidelity_store_expected(path, spec))
}

fn create_fidelity_store_expected(
    path: &Path,
    spec: &FidelityStore,
) -> Result<FidelityWriteReport, WriterError> {
    validate_spec(spec)?;
    create_flat_store(
        path,
        &StoreInput {
            store_name: &spec.store_name,
            folder_name: &spec.folder_name,
            record_key: spec.record_key,
            message: &spec.message,
            associated: false,
        },
        &[&spec.message],
        None,
        &NEVER_INTERRUPTED,
        None,
    )
}

/// Create a deterministic PST part containing multiple messages in one source folder.
///
/// Nested and multiple folder paths are rejected until the hierarchy allocator is
/// selected by `create_mail_store`; no path component is silently flattened.
pub fn create_mail_store(
    path: impl AsRef<Path>,
    spec: &MailStoreSpec,
) -> Result<FidelityWriteReport, WriterError> {
    let path = path.as_ref();
    with_writer_stack(|| create_mail_store_expected(path, spec, &NEVER_INTERRUPTED, None))
}

/// Create a deterministic PST part while honoring an operator interruption.
pub fn create_mail_store_interruptible(
    path: impl AsRef<Path>,
    spec: &MailStoreSpec,
    interrupted: &AtomicBool,
) -> Result<FidelityWriteReport, WriterError> {
    let path = path.as_ref();
    with_writer_stack(|| create_mail_store_expected(path, spec, interrupted, None))
}

/// Create a PST part with validators supervised by the PSTForge executable.
pub fn create_mail_store_supervised(
    path: impl AsRef<Path>,
    spec: &MailStoreSpec,
    interrupted: &AtomicBool,
    supervisor_executable: &Path,
) -> Result<FidelityWriteReport, WriterError> {
    let path = path.as_ref();
    with_writer_stack(|| {
        create_mail_store_expected(path, spec, interrupted, Some(supervisor_executable))
    })
}

fn create_mail_store_expected(
    path: &Path,
    spec: &MailStoreSpec,
    interrupted: &AtomicBool,
    supervisor_executable: Option<&Path>,
) -> Result<FidelityWriteReport, WriterError> {
    check_interrupted(interrupted)?;
    validate_mail_store_input(spec)?;
    let first_folder = spec
        .folders
        .iter()
        .filter(|folder| !folder.messages.is_empty() || !folder.associated_messages.is_empty())
        .min_by_key(|folder| (folder.location, &folder.path))
        .ok_or_else(|| {
            WriterError::InvalidStructure("mail store must contain at least one message".to_owned())
        })?;
    let (first, associated) = first_folder
        .messages
        .first()
        .map(|message| (message, false))
        .or_else(|| {
            first_folder
                .associated_messages
                .first()
                .map(|message| (message, true))
        })
        .ok_or_else(|| WriterError::InvalidStructure("first mail folder is empty".to_owned()))?;
    let input = StoreInput {
        store_name: &spec.store_name,
        folder_name: first_folder
            .path
            .first()
            .map(String::as_str)
            .unwrap_or("Recovered Mail"),
        record_key: spec.record_key,
        message: first,
        associated,
    };
    let messages = spec
        .folders
        .iter()
        .flat_map(|folder| {
            folder
                .messages
                .iter()
                .chain(folder.associated_messages.iter())
        })
        .collect::<Vec<_>>();
    create_flat_store(
        path,
        &input,
        &messages,
        Some(&spec.folders),
        interrupted,
        supervisor_executable,
    )
}

/// Validate every source-controlled mail-store shape without creating output.
pub fn validate_mail_store_input(spec: &MailStoreSpec) -> Result<(), WriterError> {
    let validate = || -> Result<(), WriterError> {
        if spec.store_name.is_empty() {
            return Err(WriterError::InvalidStructure(
                "store name must be non-empty".to_owned(),
            ));
        }
        validate_unicode("store name", &spec.store_name)?;
        for component in spec.folders.iter().flat_map(|folder| &folder.path) {
            validate_unicode("folder name", component)?;
        }
        let messages = spec
            .folders
            .iter()
            .flat_map(|folder| {
                folder
                    .messages
                    .iter()
                    .chain(folder.associated_messages.iter())
            })
            .collect::<Vec<_>>();
        let fallback = spec
            .folders
            .iter()
            .flat_map(|folder| folder.path.first())
            .next()
            .map(String::as_str)
            .unwrap_or("Recovered Mail");
        let folder_plans = plan_folders(fallback, &messages, Some(&spec.folders))?;
        validate_folder_hierarchy_shapes(&folder_plans)?;
        let named_identities = collect_named_identities_many_refs(&messages);
        property_context(&named_property_map(&named_identities)?)?;
        for message in messages {
            validate_aggregate_properties(message)?;
            validate_message(message, 0)?;
            validate_message_size_bound(message)?;
        }
        Ok(())
    };
    validate().map_err(input_rejection_error)
}

fn validate_folder_hierarchy_shapes(folders: &[FolderPlan<'_>]) -> Result<(), WriterError> {
    let columns = hierarchy_columns()?;
    for folder in folders {
        let rows = folder
            .children
            .iter()
            .map(|child| {
                let child = &folders[*child];
                Ok(folder_table_row_with_unread(
                    child.node,
                    child.path.last().ok_or_else(|| {
                        WriterError::InvalidStructure("folder path is empty".to_owned())
                    })?,
                    i32::try_from(child.messages.len())
                        .map_err(|_| WriterError::ValueTooLarge("folder message count"))?,
                    folder_unread_count(&child.messages)?,
                    !child.children.is_empty(),
                    &child.container_class,
                ))
            })
            .collect::<Result<Vec<_>, WriterError>>()?;
        table_context(&columns, &rows)?;
    }
    let deleted = node(NodeIdType::NormalFolder, DELETED_FOLDER_INDEX)?;
    let deleted_plan = folders.iter().find(|folder| folder.node == deleted);
    let mut rows = match deleted_plan {
        Some(folder) => vec![folder_table_row_with_unread(
            deleted,
            folder
                .path
                .last()
                .ok_or_else(|| WriterError::InvalidStructure("folder path is empty".to_owned()))?,
            i32::try_from(folder.messages.len())
                .map_err(|_| WriterError::ValueTooLarge("folder message count"))?,
            folder_unread_count(&folder.messages)?,
            !folder.children.is_empty(),
            &folder.container_class,
        )],
        None => vec![folder_table_row(deleted, "Deleted Items", 0, false)],
    };
    for folder in folders
        .iter()
        .filter(|folder| folder.parent.is_none() && folder.node != deleted)
    {
        rows.push(folder_table_row_with_unread(
            folder.node,
            folder
                .path
                .last()
                .ok_or_else(|| WriterError::InvalidStructure("folder path is empty".to_owned()))?,
            i32::try_from(folder.messages.len())
                .map_err(|_| WriterError::ValueTooLarge("folder message count"))?,
            folder_unread_count(&folder.messages)?,
            !folder.children.is_empty(),
            &folder.container_class,
        ));
    }
    table_context(&columns, &rows)?;
    Ok(())
}

fn validate_message_size_bound(message: &MessageSpec) -> Result<(), WriterError> {
    const BASE_OVERHEAD: u64 = 128 * 1024;
    const RECIPIENT_OVERHEAD: u64 = 16 * 1024;
    const ATTACHMENT_OVERHEAD: u64 = 64 * 1024;
    let mut bytes = BASE_OVERHEAD
        .checked_add(
            u64::try_from(message.recipients.len())
                .map_err(|_| WriterError::ValueTooLarge("message size"))?
                .checked_mul(RECIPIENT_OVERHEAD)
                .ok_or(WriterError::ValueTooLarge("message size"))?,
        )
        .ok_or(WriterError::ValueTooLarge("message size"))?;
    for property in &message.spooled_properties {
        bytes = bytes
            .checked_add(property.blob.byte_len)
            .ok_or(WriterError::ValueTooLarge("message size"))?;
    }
    for attachment in &message.attachments {
        bytes = bytes
            .checked_add(ATTACHMENT_OVERHEAD)
            .ok_or(WriterError::ValueTooLarge("message size"))?;
        let content_bytes = match &attachment.content {
            AttachmentContent::Binary(data) => u64::try_from(data.len())
                .map_err(|_| WriterError::ValueTooLarge("attachment properties"))?,
            AttachmentContent::Spooled(blob) => blob.byte_len,
            AttachmentContent::Embedded(embedded) => {
                validate_message_size_bound(embedded)?;
                estimated_message_payload(embedded)?
            }
        };
        let metadata_bytes = attachment_metadata_bytes(attachment)?;
        if content_bytes
            .checked_add(metadata_bytes)
            .is_none_or(|size| size > i32::MAX as u64)
        {
            return Err(WriterError::ValueTooLarge("attachment properties"));
        }
        bytes = bytes
            .checked_add(content_bytes)
            .and_then(|value| value.checked_add(metadata_bytes))
            .ok_or(WriterError::ValueTooLarge("message size"))?;
    }
    if bytes > i32::MAX as u64 {
        return Err(WriterError::ValueTooLarge("message size"));
    }
    Ok(())
}

fn estimated_message_payload(message: &MessageSpec) -> Result<u64, WriterError> {
    let mut bytes = 128_u64 * 1024;
    for property in &message.spooled_properties {
        bytes = bytes
            .checked_add(property.blob.byte_len)
            .ok_or(WriterError::ValueTooLarge("embedded message size"))?;
    }
    Ok(bytes)
}

fn attachment_metadata_bytes(attachment: &AttachmentSpec) -> Result<u64, WriterError> {
    let mut bytes = 20_u64;
    for value in [
        Some(&attachment.filename),
        Some(&attachment.filename),
        attachment.mime_type.as_ref(),
        attachment.content_id.as_ref(),
        attachment.content_location.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        bytes = bytes
            .checked_add(
                u64::try_from(unicode_payload_len(value)?)
                    .map_err(|_| WriterError::ValueTooLarge("attachment properties"))?,
            )
            .ok_or(WriterError::ValueTooLarge("attachment properties"))?;
    }
    Ok(bytes)
}

fn input_rejection_error(error: WriterError) -> WriterError {
    match error {
        WriterError::InvalidStructure(detail) => WriterError::InputRejected(detail),
        WriterError::ValueTooLarge(name) => WriterError::InputRejected(name.to_owned()),
        other => other,
    }
}

fn create_flat_store(
    path: &Path,
    spec: &StoreInput<'_>,
    messages: &[&MessageSpec],
    folders: Option<&[MailFolderSpec]>,
    interrupted: &AtomicBool,
    validator_supervisor: Option<&Path>,
) -> Result<FidelityWriteReport, WriterError> {
    check_interrupted(interrupted)?;
    let folder_plans = plan_folders(spec.folder_name, messages, folders)?;
    let top_level_messages = folder_plans
        .iter()
        .flat_map(|folder| {
            folder
                .messages
                .iter()
                .copied()
                .map(|message| (message, folder.node, false))
                .chain(
                    folder
                        .associated_messages
                        .iter()
                        .copied()
                        .map(|message| (message, folder.node, true)),
                )
        })
        .collect::<Vec<_>>();
    let messages = top_level_messages
        .iter()
        .map(|(message, _, _)| *message)
        .collect::<Vec<_>>();
    let mut unsupported_properties = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let index =
            u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("message count"))?;
        let path = (messages.len() != 1)
            .then_some(index)
            .into_iter()
            .collect::<Vec<_>>();
        unsupported_properties.extend(collect_unsupported_properties(message, &path)?);
    }
    let report = FidelityWriteReport {
        unsupported_properties,
    };
    match path.symlink_metadata() {
        Ok(_) => return Err(WriterError::OutputExists(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(WriterError::Io(error)),
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_directory = std::fs::File::open(parent)?;
    let mut temporary = PublicationTemporary::new(parent)?;
    let file = &mut temporary.file;
    file.set_len(INITIAL_FILE_EOF)?;
    let mut allocation_cursor = FIRST_DATA;
    let mut written = Vec::new();
    let mut streamed_subnodes = Vec::new();

    let root_folder = NID_ROOT_FOLDER;
    let ipm_folder = node(NodeIdType::NormalFolder, IPM_FOLDER_INDEX)?;
    let search_root = node(NodeIdType::NormalFolder, SEARCH_ROOT_INDEX)?;
    let deleted_folder = node(NodeIdType::NormalFolder, DELETED_FOLDER_INDEX)?;
    let spam_search = node(NodeIdType::SearchFolder, SPAM_SEARCH_INDEX)?;
    let hierarchy_columns = hierarchy_columns()?;
    let contents_columns = contents_columns()?;
    let associated_columns = associated_columns()?;
    let search_contents_columns = search_contents_columns()?;
    let receive_folder_columns = receive_folder_columns()?;
    let outgoing_queue_columns = outgoing_queue_columns()?;
    let contents_index_columns = contents_index_columns()?;
    let search_index_columns = search_index_columns()?;
    let attachment_index_columns = attachment_index_columns()?;
    if messages.is_empty() {
        return Err(WriterError::InvalidStructure(
            "mail store must contain at least one message".to_owned(),
        ));
    }
    let named_identities = collect_named_identities_many_refs(&messages);
    let mut next_block_index = 28_u64;
    let mut next_value_node = 0x4_0000_u32;
    let mut contents_rows = Vec::with_capacity(messages.len());
    let mut associated_rows = Vec::with_capacity(messages.len());
    let mut top_nodes = Vec::with_capacity(messages.len());
    let mut single_message = None;
    for (message_position, message_spec) in messages.iter().enumerate() {
        check_interrupted(interrupted)?;
        let index = u32::try_from(message_position)
            .map_err(|_| WriterError::ValueTooLarge("message count"))?;
        let associated = top_level_messages[message_position].2;
        let message_node = node(
            if associated {
                NodeIdType::AssociatedMessage
            } else {
                NodeIdType::NormalMessage
            },
            MESSAGE_INDEX
                .checked_add(index)
                .ok_or(WriterError::ValueTooLarge("message node"))?,
        )?;
        let (property_block, recipient_block, attachment_block, subnode_block) = if index == 0 {
            (
                leaf_bid(12)?,
                leaf_bid(17)?,
                leaf_bid(18)?,
                internal_bid(27)?,
            )
        } else {
            (
                take_block_id(&mut next_block_index, false)?,
                take_block_id(&mut next_block_index, false)?,
                take_block_id(&mut next_block_index, false)?,
                take_block_id(&mut next_block_index, true)?,
            )
        };
        let stream_message = messages.len() > 1 || message_requires_streaming(message_spec);
        let mut block_stream = stream_message.then_some(BlockStream {
            file: &mut *file,
            cursor: &mut allocation_cursor,
            written: &mut written,
            interrupted,
        });
        let message = build_message_blocks(
            message_spec,
            associated,
            message_record_key(spec.record_key, message_node),
            &named_identities,
            recipient_block,
            attachment_block,
            &mut next_block_index,
            &mut next_value_node,
            block_stream.as_mut(),
        )?;
        if associated {
            associated_rows.push(associated_message_table_row(
                message_node,
                message_spec,
                &associated_columns,
            ));
        } else {
            contents_rows.push(message_table_row(
                message_node,
                message_spec,
                spec.record_key,
                message.record_key,
                message.message_size,
                &contents_columns,
            )?);
        }
        top_nodes.push(TopMessageNode {
            node: message_node,
            property_block,
            subnode_block,
            parent: top_level_messages[message_position].1,
        });
        let built = BuiltTopMessage {
            property_block,
            recipient_block,
            attachment_block,
            subnode_block,
            shared_table_blocks: index == 0,
            message,
        };
        if !stream_message {
            single_message = Some(built);
        } else {
            let message_blocks = built_message_block_specs(built);
            collect_subnode_ids(&message_blocks, &mut streamed_subnodes);
            let stream = block_stream.as_mut().ok_or_else(|| {
                WriterError::InvalidStructure("message block stream is missing".to_owned())
            })?;
            for block in message_blocks {
                stream.emit(block)?;
            }
        }
    }
    let mut folder_blocks = Vec::new();
    let mut top_folders = Vec::with_capacity(folder_plans.len());
    let mut row_start = 0_usize;
    let mut associated_row_start = 0_usize;
    for (index, folder) in folder_plans.iter().enumerate() {
        check_interrupted(interrupted)?;
        let unread_count = folder_unread_count(&folder.messages)?;
        let is_deleted = folder.node == deleted_folder;
        let property_block = if is_deleted {
            leaf_bid(8)?
        } else if index == 0 {
            leaf_bid(10)?
        } else {
            take_block_id(&mut next_block_index, false)?
        };
        if !is_deleted {
            folder_blocks.push(BlockSpec {
                id: property_block,
                payload: BlockPayload::Data(property_context(&folder_properties_with_unread(
                    folder.path.last().ok_or_else(|| {
                        WriterError::InvalidStructure("folder path is empty".to_owned())
                    })?,
                    i32::try_from(folder.messages.len())
                        .map_err(|_| WriterError::ValueTooLarge("folder message count"))?,
                    unread_count,
                    !folder.children.is_empty(),
                    &folder.container_class,
                ))?),
                ref_count: 2,
            });
        }

        let hierarchy_block = if folder.children.is_empty() {
            leaf_bid(9)?
        } else {
            let block = take_block_id(&mut next_block_index, false)?;
            let rows = folder
                .children
                .iter()
                .map(|child| {
                    let child = &folder_plans[*child];
                    Ok(folder_table_row_with_unread(
                        child.node,
                        child.path.last().ok_or_else(|| {
                            WriterError::InvalidStructure("folder path is empty".to_owned())
                        })?,
                        i32::try_from(child.messages.len())
                            .map_err(|_| WriterError::ValueTooLarge("folder message count"))?,
                        folder_unread_count(&child.messages)?,
                        !child.children.is_empty(),
                        &child.container_class,
                    ))
                })
                .collect::<Result<Vec<_>, WriterError>>()?;
            folder_blocks.push(BlockSpec {
                id: block,
                payload: BlockPayload::Data(table_context(&hierarchy_columns, &rows)?),
                ref_count: 2,
            });
            block
        };

        let row_end = row_start
            .checked_add(folder.messages.len())
            .ok_or(WriterError::ValueTooLarge("folder row range"))?;
        let rows = &contents_rows[row_start..row_end];
        row_start = row_end;
        let (contents_block, contents_subnode) = if rows.is_empty() {
            (leaf_bid(5)?, None)
        } else if rows.len() == 1 {
            let block = if index == 0 {
                leaf_bid(11)?
            } else {
                take_block_id(&mut next_block_index, false)?
            };
            folder_blocks.push(BlockSpec {
                id: block,
                payload: BlockPayload::Data(table_context(&contents_columns, rows)?),
                ref_count: 2,
            });
            (block, None)
        } else {
            let contents = table_context_external(&contents_columns, rows, &mut next_block_index)?;
            let data = contents.data_block;
            let subnode = contents.subnode_block;
            folder_blocks.extend(contents.blocks);
            (data, Some(subnode))
        };
        let associated_row_end = associated_row_start
            .checked_add(folder.associated_messages.len())
            .ok_or(WriterError::ValueTooLarge("associated folder row range"))?;
        let rows = &associated_rows[associated_row_start..associated_row_end];
        associated_row_start = associated_row_end;
        let (associated_block, associated_subnode) = if rows.is_empty() {
            (leaf_bid(13)?, None)
        } else if rows.len() == 1 {
            let block = take_block_id(&mut next_block_index, false)?;
            folder_blocks.push(BlockSpec {
                id: block,
                payload: BlockPayload::Data(table_context(&associated_columns, rows)?),
                ref_count: 2,
            });
            (block, None)
        } else {
            let associated =
                table_context_external(&associated_columns, rows, &mut next_block_index)?;
            let data = associated.data_block;
            let subnode = associated.subnode_block;
            folder_blocks.extend(associated.blocks);
            (data, Some(subnode))
        };
        let parent = folder.parent.map_or_else(
            || match folder.location {
                MailFolderLocation::StoreRoot => root_folder,
                MailFolderLocation::IpmSubtree => ipm_folder,
            },
            |parent| folder_plans[parent].node,
        );
        top_folders.push(TopFolderNode {
            node: folder.node,
            parent,
            property_block,
            hierarchy_block,
            contents_block,
            contents_subnode,
            associated_block,
            associated_subnode,
        });
    }
    let deleted_plan = folder_plans
        .iter()
        .find(|folder| folder.node == deleted_folder);
    let mut ipm_rows = match deleted_plan {
        Some(folder) => vec![folder_table_row_with_unread(
            deleted_folder,
            folder
                .path
                .last()
                .ok_or_else(|| WriterError::InvalidStructure("folder path is empty".to_owned()))?,
            i32::try_from(folder.messages.len())
                .map_err(|_| WriterError::ValueTooLarge("folder message count"))?,
            folder_unread_count(&folder.messages)?,
            !folder.children.is_empty(),
            &folder.container_class,
        )],
        None => vec![folder_table_row(deleted_folder, "Deleted Items", 0, false)],
    };
    for folder in folder_plans.iter().filter(|folder| {
        folder.location == MailFolderLocation::IpmSubtree
            && folder.parent.is_none()
            && folder.node != deleted_folder
    }) {
        ipm_rows.push(folder_table_row_with_unread(
            folder.node,
            folder
                .path
                .last()
                .ok_or_else(|| WriterError::InvalidStructure("folder path is empty".to_owned()))?,
            i32::try_from(folder.messages.len())
                .map_err(|_| WriterError::ValueTooLarge("folder message count"))?,
            folder_unread_count(&folder.messages)?,
            !folder.children.is_empty(),
            &folder.container_class,
        ));
    }
    let empty_contents = folder_plans
        .iter()
        .filter(|folder| folder.node != deleted_folder && folder.messages.is_empty())
        .count();
    let empty_hierarchies = folder_plans
        .iter()
        .filter(|folder| folder.node != deleted_folder && folder.children.is_empty())
        .count();
    let deleted_uses_shared_contents = deleted_plan.is_none_or(|folder| folder.messages.is_empty());
    let deleted_uses_shared_hierarchy =
        deleted_plan.is_none_or(|folder| folder.children.is_empty());
    let mut root_rows = vec![
        folder_table_row(ipm_folder, "Top of Personal Folders", 0, true),
        folder_table_row(search_root, "Search Root", 0, false),
        folder_table_row(spam_search, "SPAM Search Folder 2", 0, false),
    ];
    for folder in folder_plans.iter().filter(|folder| {
        folder.location == MailFolderLocation::StoreRoot && folder.parent.is_none()
    }) {
        root_rows.push(folder_table_row_with_unread(
            folder.node,
            folder
                .path
                .last()
                .ok_or_else(|| WriterError::InvalidStructure("folder path is empty".to_owned()))?,
            i32::try_from(folder.messages.len())
                .map_err(|_| WriterError::ValueTooLarge("folder message count"))?,
            folder_unread_count(&folder.messages)?,
            !folder.children.is_empty(),
            &folder.container_class,
        ));
    }
    // BBT cRef includes one ownership count beyond the NBT references.
    let shared_ref_count = |base: usize, extra: usize| {
        u16::try_from(base.saturating_add(extra))
            .map_err(|_| WriterError::ValueTooLarge("shared block reference count"))
    };
    let mut blocks = vec![
        BlockSpec {
            id: leaf_bid(1)?,
            payload: BlockPayload::Data(property_context(&store_properties(
                spec,
                ipm_folder,
                deleted_folder,
                search_root,
            )?)?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(2)?,
            payload: BlockPayload::Data(property_context(&named_property_map(&named_identities)?)?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(3)?,
            payload: BlockPayload::Data(property_context(&folder_properties("", 0, true))?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(4)?,
            payload: BlockPayload::Data(table_context(&hierarchy_columns, &root_rows)?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(5)?,
            payload: BlockPayload::Data(table_context(&contents_columns, &[])?),
            ref_count: shared_ref_count(
                5 + usize::from(deleted_uses_shared_contents),
                empty_contents,
            )?,
        },
        BlockSpec {
            id: leaf_bid(6)?,
            payload: BlockPayload::Data(property_context(&folder_properties(
                "Top of Personal Folders",
                0,
                true,
            ))?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(7)?,
            payload: BlockPayload::Data(table_context(&hierarchy_columns, &ipm_rows)?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(8)?,
            payload: BlockPayload::Data(property_context(&match deleted_plan {
                Some(folder) => folder_properties_with_unread(
                    folder.path.last().ok_or_else(|| {
                        WriterError::InvalidStructure("folder path is empty".to_owned())
                    })?,
                    i32::try_from(folder.messages.len())
                        .map_err(|_| WriterError::ValueTooLarge("folder message count"))?,
                    folder_unread_count(&folder.messages)?,
                    !folder.children.is_empty(),
                    &folder.container_class,
                ),
                None => folder_properties("Deleted Items", 0, false),
            })?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(9)?,
            payload: BlockPayload::Data(table_context(&hierarchy_columns, &[])?),
            ref_count: shared_ref_count(
                3 + usize::from(deleted_uses_shared_hierarchy),
                empty_hierarchies,
            )?,
        },
        BlockSpec {
            id: leaf_bid(13)?,
            payload: BlockPayload::Data(table_context(&associated_columns, &[])?),
            ref_count: shared_ref_count(
                6,
                folder_plans
                    .iter()
                    .filter(|folder| {
                        folder.node != deleted_folder && folder.associated_messages.is_empty()
                    })
                    .count(),
            )?,
        },
        BlockSpec {
            id: leaf_bid(14)?,
            payload: BlockPayload::Data(property_context(&folder_properties(
                "Search Root",
                0,
                false,
            ))?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(15)?,
            payload: BlockPayload::Data(property_context(&folder_properties(
                "SPAM Search Folder 2",
                0,
                false,
            ))?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(16)?,
            payload: BlockPayload::Data(table_context(&search_contents_columns, &[])?),
            ref_count: 3,
        },
        BlockSpec {
            id: leaf_bid(19)?,
            payload: BlockPayload::Data(u32::from(spam_search).to_le_bytes().to_vec()),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(20)?,
            payload: BlockPayload::Data(table_context(
                &receive_folder_columns,
                &[TableRowSpec {
                    id: NodeId::from(1_u32),
                    values: vec![
                        (0x001A, PropertyValue::Unicode(String::new())),
                        (
                            0x6605,
                            PropertyValue::Integer32(
                                i32::try_from(u32::from(root_folder))
                                    .map_err(|_| WriterError::ValueTooLarge("receive folder id"))?,
                            ),
                        ),
                    ],
                }],
            )?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(21)?,
            payload: BlockPayload::Data(table_context(&outgoing_queue_columns, &[])?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(22)?,
            payload: BlockPayload::Data(table_context(&contents_index_columns, &[])?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(23)?,
            payload: BlockPayload::Data(table_context(&search_index_columns, &[])?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(24)?,
            payload: BlockPayload::Data(table_context(&attachment_index_columns, &[])?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(25)?,
            payload: BlockPayload::Data(property_context(&[(
                0x660B,
                PropertyValue::Integer32(0),
            )])?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(26)?,
            payload: BlockPayload::Data(hierarchy_map()),
            ref_count: 2,
        },
    ];
    blocks.extend(folder_blocks);
    if let Some(message) = single_message {
        blocks.extend(built_message_block_specs(message));
    }
    blocks.sort_by_key(|block| u64::from(block.id));

    written.extend(write_blocks(
        &mut *file,
        &blocks,
        &mut allocation_cursor,
        interrupted,
    )?);
    written.sort_by_key(|block| u64::from(block.id));
    let page_offset = align_up(allocation_cursor, PAGE_SIZE);
    let (bbt, nbt_offset, next_page_id) = write_bbt(&mut *file, page_offset, 0x100, &written)?;
    let nodes = node_entries(
        root_folder,
        ipm_folder,
        search_root,
        deleted_folder,
        spam_search,
        &top_folders,
        &top_nodes,
    )?;
    let (nbt, allocated_end, next_page_id) =
        write_nbt(&mut *file, nbt_offset, next_page_id, &nodes)?;

    let dynamic_allocation = allocated_end > INITIAL_FILE_EOF;
    let file_eof = allocation_file_eof(allocated_end)?;
    file.set_len(file_eof)?;
    if !dynamic_allocation {
        write_fixed_pages(&mut *file, allocated_end, UnicodePageId::from(next_page_id))?;
    }
    write_header(
        &mut *file,
        nbt,
        bbt,
        allocated_end,
        UnicodePageId::from(next_page_id),
        leaf_bid(next_block_index)?,
        nid_counters(&nodes, &blocks, &streamed_subnodes)?,
        dynamic_allocation,
    )?;
    file.sync_all()?;
    check_interrupted(interrupted)?;
    let validated_path = PathBuf::from(format!(
        "/proc/{}/fd/{}",
        std::process::id(),
        temporary.file.as_raw_fd()
    ));
    if dynamic_allocation {
        (|| -> Result<(), WriterError> {
            let mut pst = UnicodePstFile::open(&validated_path)?;
            let mut transaction = pst.lock()?;
            transaction.flush()?;
            drop(transaction);
            drop(pst);
            temporary.file.sync_all()?;
            Ok(())
        })()
        .map_err(completed_validation_error)?;
    }
    check_interrupted(interrupted)?;
    if !spec.associated {
        validate_completed_store(
            &validated_path,
            spec,
            top_level_messages[0].1,
            &named_identities,
        )
        .map_err(completed_validation_error)?;
    }
    check_interrupted(interrupted)?;
    validate_completed_folder_store(&validated_path, spec.record_key, &folder_plans)
        .map_err(completed_validation_error)?;
    check_interrupted(interrupted)?;
    validate_with_independent_readers(
        &validated_path,
        &mut temporary,
        interrupted,
        validator_supervisor,
    )?;
    check_interrupted(interrupted)?;
    publish_noclobber(
        temporary.source_name(),
        &temporary.directory,
        &parent_directory,
        path,
    )?;
    sync_published_directory(path, &parent_directory)?;
    verify_published_destination(path, &temporary.file)?;
    Ok(report)
}

fn completed_validation_error(error: WriterError) -> WriterError {
    WriterError::CompletedValidation(error.to_string())
}

fn collect_unsupported_properties(
    message: &MessageSpec,
    message_path: &[u32],
) -> Result<Vec<UnsupportedPropertyRecord>, WriterError> {
    let mut properties = message
        .unsupported_properties
        .iter()
        .cloned()
        .map(|property| UnsupportedPropertyRecord {
            message_path: message_path.to_vec(),
            property,
        })
        .collect::<Vec<_>>();
    for (index, attachment) in message.attachments.iter().enumerate() {
        if let AttachmentContent::Embedded(embedded) = &attachment.content {
            let mut child_path = message_path.to_vec();
            child_path.push(
                u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("attachment count"))?,
            );
            properties.extend(collect_unsupported_properties(embedded, &child_path)?);
        }
    }
    Ok(properties)
}

fn sync_published_directory(destination: &Path, parent: &std::fs::File) -> Result<(), WriterError> {
    parent
        .sync_all()
        .map_err(|source| WriterError::PublishedDurability {
            path: destination.to_path_buf(),
            source,
        })
}

const VALIDATOR_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_VALIDATOR_DIAGNOSTIC_BYTES: usize = 64 * 1024;

struct ValidatorOutput {
    success: bool,
    timed_out: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

fn capture_bounded(mut input: impl Read) -> io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let available = MAX_VALIDATOR_DIAGNOSTIC_BYTES.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(available)]);
        truncated |= read > available;
    }
    Ok((retained, truncated))
}

fn run_validator(
    command: &mut Command,
    timeout: Duration,
    interrupted: &AtomicBool,
) -> io::Result<ValidatorOutput> {
    command.process_group(0);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("validator stdout pipe is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("validator stderr pipe is unavailable"))?;
    let (stdout_sender, stdout_receiver) = std::sync::mpsc::channel();
    let (stderr_sender, stderr_receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = stdout_sender.send(capture_bounded(stdout));
    });
    thread::spawn(move || {
        let _ = stderr_sender.send(capture_bounded(stderr));
    });
    let pid = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or_else(|| io::Error::other("validator process ID is out of range"))?;
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut stdout_result = None;
    let mut stderr_result = None;
    let (timed_out, was_interrupted) = loop {
        if status.is_none() {
            status = child.try_wait()?;
        }
        if stdout_result.is_none() {
            stdout_result = stdout_receiver.try_recv().ok();
        }
        if stderr_result.is_none() {
            stderr_result = stderr_receiver.try_recv().ok();
        }
        if status.is_some() && stdout_result.is_some() && stderr_result.is_some() {
            break (false, false);
        }
        let signal_observed = interrupted.load(Ordering::Relaxed);
        if signal_observed || Instant::now() >= deadline {
            if let Err(error) =
                rustix::process::kill_process_group(pid, rustix::process::Signal::KILL)
                && error != rustix::io::Errno::SRCH
            {
                return Err(error.into());
            }
            if status.is_none() {
                status = Some(child.wait()?);
            }
            let drain_deadline = Instant::now() + Duration::from_secs(1);
            while (stdout_result.is_none() || stderr_result.is_none())
                && Instant::now() < drain_deadline
            {
                if stdout_result.is_none() {
                    stdout_result = stdout_receiver.try_recv().ok();
                }
                if stderr_result.is_none() {
                    stderr_result = stderr_receiver.try_recv().ok();
                }
                thread::sleep(Duration::from_millis(10));
            }
            stdout_result.get_or_insert_with(|| Ok((Vec::new(), true)));
            stderr_result.get_or_insert_with(|| Ok((Vec::new(), true)));
            break (!signal_observed, signal_observed);
        }
        thread::sleep(Duration::from_millis(10));
    };
    let status = status.ok_or_else(|| io::Error::other("validator status is unavailable"))?;
    let (stdout, stdout_truncated) = stdout_result
        .ok_or_else(|| io::Error::other("validator stdout result is unavailable"))??;
    let (stderr, stderr_truncated) = stderr_result
        .ok_or_else(|| io::Error::other("validator stderr result is unavailable"))??;
    if was_interrupted {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "PST validation was interrupted",
        ));
    }
    Ok(ValidatorOutput {
        success: status.success() && !timed_out,
        timed_out,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

fn validate_with_independent_readers(
    path: &Path,
    temporary: &mut PublicationTemporary,
    interrupted: &AtomicBool,
    supervisor_executable: Option<&Path>,
) -> Result<(), WriterError> {
    let mut pffinfo = validator_command("pffinfo", supervisor_executable);
    pffinfo.arg(path);
    let outcome =
        run_validator(&mut pffinfo, VALIDATOR_TIMEOUT, interrupted).map_err(|source| {
            if source.kind() == io::ErrorKind::Interrupted {
                WriterError::Interrupted
            } else {
                WriterError::IndependentValidatorIo {
                    tool: "pffinfo",
                    source,
                }
            }
        })?;
    if !outcome.success {
        let evidence = temporary.retain_validation_failure("pffinfo", &outcome)?;
        return Err(WriterError::IndependentValidation {
            tool: "pffinfo",
            evidence,
        });
    }

    let output =
        temporary
            .validator_scratch()
            .map_err(|source| WriterError::IndependentValidatorIo {
                tool: "readpst scratch directory",
                source,
            })?;
    let output_path =
        output
            .path()
            .canonicalize()
            .map_err(|source| WriterError::IndependentValidatorIo {
                tool: "readpst scratch directory",
                source,
            })?;
    let mut readpst = validator_command("readpst", supervisor_executable);
    readpst.args(["-q", "-r", "-o"]).arg(output_path).arg(path);
    let outcome =
        run_validator(&mut readpst, VALIDATOR_TIMEOUT, interrupted).map_err(|source| {
            if source.kind() == io::ErrorKind::Interrupted {
                WriterError::Interrupted
            } else {
                WriterError::IndependentValidatorIo {
                    tool: "readpst",
                    source,
                }
            }
        })?;
    if !outcome.success {
        let evidence = temporary.retain_validation_failure("readpst", &outcome)?;
        return Err(WriterError::IndependentValidation {
            tool: "readpst",
            evidence,
        });
    }
    Ok(())
}

fn validator_command(tool: &'static str, supervisor_executable: Option<&Path>) -> Command {
    let Some(supervisor_executable) = supervisor_executable else {
        return Command::new(tool);
    };
    let mut command = Command::new(supervisor_executable);
    command
        .arg("__validator")
        .arg(std::process::id().to_string())
        .arg(tool)
        .arg("--");
    command
}

struct PublicationTemporary {
    file: std::fs::File,
    source_name: std::ffi::OsString,
    directory: std::fs::File,
    directory_name: std::ffi::OsString,
    parent_directory: std::fs::File,
    retain: bool,
}

impl PublicationTemporary {
    fn new(parent: &Path) -> Result<Self, WriterError> {
        let directory = tempfile::Builder::new()
            .prefix(".pstforge-")
            .tempdir_in(parent)?;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        let directory_name = directory
            .path()
            .file_name()
            .ok_or_else(|| {
                WriterError::InvalidStructure("temporary directory has no name".to_owned())
            })?
            .to_owned();
        let parent_directory = std::fs::File::open(parent)?;
        let directory_handle = std::fs::File::open(directory.path())?;
        let temporary = tempfile::NamedTempFile::new_in(directory.path())?;
        let (file, file_path) = temporary
            .keep()
            .map_err(|error| WriterError::Io(error.error))?;
        let _directory_path = directory.keep();
        let source_name = file_path
            .file_name()
            .ok_or_else(|| {
                WriterError::InvalidStructure("temporary output has no file name".to_owned())
            })?
            .to_owned();
        Ok(Self {
            file,
            source_name,
            directory: directory_handle,
            directory_name,
            parent_directory,
            retain: false,
        })
    }

    fn source_name(&self) -> &std::ffi::OsStr {
        &self.source_name
    }

    fn directory_path(&self) -> io::Result<PathBuf> {
        std::fs::read_link(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
    }

    fn validator_scratch(&self) -> io::Result<tempfile::TempDir> {
        tempfile::Builder::new()
            .prefix(".readpst-")
            .tempdir_in(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
    }

    fn retain_validation_failure(
        &mut self,
        tool: &'static str,
        outcome: &ValidatorOutput,
    ) -> Result<PathBuf, WriterError> {
        let evidence = self.directory_path()?;
        let diagnostic_path = format!(
            "/proc/self/fd/{}/validator-failure.log",
            self.directory.as_raw_fd()
        );
        let mut diagnostic = Vec::new();
        diagnostic.extend_from_slice(format!("tool: {tool}\n").as_bytes());
        diagnostic.extend_from_slice(format!("timed out: {}\n", outcome.timed_out).as_bytes());
        diagnostic.extend_from_slice(
            format!("stdout truncated: {}\n", outcome.stdout_truncated).as_bytes(),
        );
        diagnostic.extend_from_slice(
            format!("stderr truncated: {}\n", outcome.stderr_truncated).as_bytes(),
        );
        diagnostic
            .extend_from_slice(format!("stdout bytes: {}\n", outcome.stdout.len()).as_bytes());
        diagnostic
            .extend_from_slice(format!("stderr bytes: {}\n", outcome.stderr.len()).as_bytes());
        diagnostic.extend_from_slice(b"validator output redacted to protect recovered mail data\n");
        let diagnostic_file = std::fs::File::create(diagnostic_path)?;
        diagnostic_file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        (&diagnostic_file).write_all(&diagnostic)?;
        diagnostic_file.sync_all()?;
        self.directory.sync_all()?;
        self.retain = true;
        Ok(evidence)
    }
}

impl Drop for PublicationTemporary {
    fn drop(&mut self) {
        if self.retain {
            return;
        }
        let _ = rustix::fs::unlinkat(
            &self.directory,
            self.source_name(),
            rustix::fs::AtFlags::empty(),
        );
        let held = self.directory.metadata();
        let named = rustix::fs::statat(
            &self.parent_directory,
            &self.directory_name,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        );
        if let (Ok(held), Ok(named)) = (held, named)
            && held.dev() == named.st_dev
            && held.ino() == named.st_ino
        {
            let _ = rustix::fs::unlinkat(
                &self.parent_directory,
                &self.directory_name,
                rustix::fs::AtFlags::REMOVEDIR,
            );
        }
    }
}

fn verify_published_destination(
    destination: &Path,
    published: &std::fs::File,
) -> Result<(), WriterError> {
    use std::os::unix::fs::MetadataExt;

    let expected = published
        .metadata()
        .map_err(|_| WriterError::PublishedDestinationChanged(destination.to_path_buf()))?;
    let actual = destination
        .symlink_metadata()
        .map_err(|_| WriterError::PublishedDestinationChanged(destination.to_path_buf()))?;
    if expected.dev() != actual.dev() || expected.ino() != actual.ino() {
        return Err(WriterError::PublishedDestinationChanged(
            destination.to_path_buf(),
        ));
    }
    Ok(())
}

fn publish_noclobber(
    source_name: &std::ffi::OsStr,
    source_directory: &std::fs::File,
    destination_directory: &std::fs::File,
    destination: &Path,
) -> Result<(), WriterError> {
    use rustix::{
        fs::{RenameFlags, renameat_with},
        io::Errno,
    };

    let destination_name = destination
        .file_name()
        .ok_or_else(|| WriterError::InvalidStructure("output has no file name".to_owned()))?;

    match renameat_with(
        source_directory,
        source_name,
        destination_directory,
        destination_name,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(WriterError::OutputExists(destination.to_path_buf())),
        Err(Errno::NOSYS | Errno::INVAL | Errno::NOTSUP) => Err(WriterError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-replace rename is unsupported by this kernel or filesystem",
        ))),
        Err(error) => Err(WriterError::Io(error.into())),
    }
}

fn validate_spec(spec: &FidelityStore) -> Result<(), WriterError> {
    for (name, value) in [
        ("store name", &spec.store_name),
        ("folder name", &spec.folder_name),
    ] {
        if value.is_empty() {
            return Err(WriterError::InvalidStructure(format!(
                "{name} must be non-empty"
            )));
        }
        let units = value.encode_utf16().count();
        if units > 2048 {
            return Err(WriterError::ValueTooLarge(name));
        }
    }
    validate_aggregate_properties(&spec.message)?;
    validate_message(&spec.message, 0)
}

fn validate_aggregate_properties(message: &MessageSpec) -> Result<(), WriterError> {
    fn visit<'a>(
        message: &'a MessageSpec,
        identities: &mut BTreeSet<(NamedPropertySet, &'a NamedPropertyName)>,
        named_occurrences: &mut usize,
        unsupported_occurrences: &mut usize,
    ) -> Result<(), WriterError> {
        if message.recipients.len() > MAX_FIDELITY_COLLECTION_ITEMS {
            return Err(WriterError::ValueTooLarge("recipient count"));
        }
        let recipient_bytes = message
            .recipients
            .iter()
            .try_fold(0_usize, |total, recipient| {
                let email_size = unicode_payload_len(&recipient.email_address)?;
                total
                    .checked_add(unicode_payload_len(&recipient.display_name)?)
                    .and_then(|total| total.checked_add(email_size))
                    .ok_or(WriterError::ValueTooLarge("aggregate recipient metadata"))
            })?;
        validate_payload_len("aggregate recipient metadata", recipient_bytes)?;
        for kind in [RecipientKind::To, RecipientKind::Cc, RecipientKind::Bcc] {
            let mut count = 0_usize;
            let display_bytes = message
                .recipients
                .iter()
                .filter(|recipient| recipient.kind == kind)
                .try_fold(0_usize, |total, recipient| {
                    let separator = if count == 0 { 0 } else { 4 };
                    let display_size = unicode_payload_len(&recipient.display_name)?;
                    count = count
                        .checked_add(1)
                        .ok_or(WriterError::ValueTooLarge("recipient count"))?;
                    total
                        .checked_add(separator)
                        .and_then(|total| total.checked_add(display_size))
                        .ok_or(WriterError::ValueTooLarge("display recipient property"))
                })?;
            validate_payload_len("display recipient property", display_bytes)?;
        }
        validate_recipient_table_shape(message)?;

        *named_occurrences = named_occurrences
            .checked_add(message.named_properties.len())
            .ok_or(WriterError::ValueTooLarge("named-property count"))?;
        if *named_occurrences > MAX_FIDELITY_COLLECTION_ITEMS {
            return Err(WriterError::ValueTooLarge("named-property count"));
        }
        identities.extend(
            message
                .named_properties
                .iter()
                .map(|property| (property.set, &property.name)),
        );
        if message.attachments.len() > MAX_FIDELITY_COLLECTION_ITEMS {
            return Err(WriterError::ValueTooLarge("attachment count"));
        }
        validate_attachment_table_shape(message)?;
        for attachment in &message.attachments {
            validate_attachment_property_context_shape(attachment)?;
        }
        *unsupported_occurrences = unsupported_occurrences
            .checked_add(message.unsupported_properties.len())
            .ok_or(WriterError::ValueTooLarge("unsupported-property count"))?;
        if *unsupported_occurrences > MAX_FIDELITY_COLLECTION_ITEMS {
            return Err(WriterError::ValueTooLarge("unsupported-property count"));
        }
        for attachment in &message.attachments {
            if let AttachmentContent::Embedded(child) = &attachment.content {
                visit(
                    child,
                    identities,
                    named_occurrences,
                    unsupported_occurrences,
                )?;
            }
        }
        Ok(())
    }

    let mut identities = BTreeSet::new();
    let mut named_occurrences = 0_usize;
    let mut unsupported_occurrences = 0_usize;
    visit(
        message,
        &mut identities,
        &mut named_occurrences,
        &mut unsupported_occurrences,
    )?;
    validate_payload_len(
        "named-property entry stream",
        identities
            .len()
            .checked_mul(8)
            .ok_or(WriterError::ValueTooLarge("named-property entry stream"))?,
    )?;
    let string_bytes = identities.iter().try_fold(0_usize, |total, (_, name)| {
        let NamedPropertyName::String(name) = name else {
            return Ok(total);
        };
        let entry = unicode_payload_len(name)?
            .checked_add(4)
            .ok_or(WriterError::ValueTooLarge("named-property string stream"))?;
        let padded = entry
            .checked_add(3)
            .map(|size| size / 4 * 4)
            .ok_or(WriterError::ValueTooLarge("named-property string stream"))?;
        total
            .checked_add(padded)
            .ok_or(WriterError::ValueTooLarge("named-property string stream"))
    })?;
    validate_payload_len("named-property string stream", string_bytes)?;
    let custom_guids = identities
        .iter()
        .filter_map(|(set, _)| match set {
            NamedPropertySet::Guid(guid) => Some(guid),
            NamedPropertySet::Mapi | NamedPropertySet::PublicStrings => None,
        })
        .collect::<BTreeSet<_>>();
    let custom_guid_bytes = custom_guids
        .len()
        .checked_mul(16)
        .ok_or(WriterError::ValueTooLarge("named-property GUID stream"))?;
    validate_payload_len("named-property GUID stream", custom_guid_bytes)?;
    validate_named_property_map_shape(&identities, &custom_guids, string_bytes)
}

fn validate_named_property_map_shape(
    identities: &BTreeSet<(NamedPropertySet, &NamedPropertyName)>,
    custom_guids: &BTreeSet<&[u8; 16]>,
    string_bytes: usize,
) -> Result<(), WriterError> {
    let mut bucket_lengths = [0_usize; 251];
    for (set, name) in identities {
        let guid = match set {
            NamedPropertySet::Mapi => 1_u16,
            NamedPropertySet::PublicStrings => 2_u16,
            NamedPropertySet::Guid(value) => custom_guids
                .iter()
                .position(|candidate| *candidate == value)
                .and_then(|position| u16::try_from(position).ok())
                .and_then(|position| position.checked_add(3))
                .ok_or(WriterError::ValueTooLarge("named-property GUID count"))?,
        };
        let (hash_identifier, kind) = match name {
            NamedPropertyName::Numeric(identifier) => (*identifier, 0_u16),
            NamedPropertyName::String(name) => {
                let encoded = unicode_bytes(name)?;
                (crate::crc::compute_crc(0, &encoded), 1_u16)
            }
        };
        let guid_and_kind = (guid << 1) | kind;
        let bucket = usize::try_from((hash_identifier ^ u32::from(guid_and_kind)) % 251)
            .map_err(|_| WriterError::ValueTooLarge("named-property bucket"))?;
        bucket_lengths[bucket] = bucket_lengths[bucket]
            .checked_add(8)
            .ok_or(WriterError::ValueTooLarge("named-property bucket"))?;
    }

    let mut empty_properties = vec![
        (0x0001, PropertyValue::Integer32(251)),
        (0x0002, PropertyValue::Binary(Vec::new())),
        (0x0003, PropertyValue::Binary(Vec::new())),
        (0x0004, PropertyValue::Binary(Vec::new())),
    ];
    empty_properties.extend(
        bucket_lengths
            .iter()
            .enumerate()
            .filter(|(_, length)| **length != 0)
            .map(|(bucket, _)| (0x1000 + bucket as u16, PropertyValue::Binary(Vec::new()))),
    );
    let base_size = property_context(&empty_properties)?.len();
    let entry_bytes = identities
        .len()
        .checked_mul(8)
        .ok_or(WriterError::ValueTooLarge("named-property entry stream"))?;
    let guid_bytes = if custom_guids.is_empty() {
        16
    } else {
        custom_guids
            .len()
            .checked_mul(16)
            .ok_or(WriterError::ValueTooLarge("named-property GUID stream"))?
    };
    let mut allocation_lengths = vec![guid_bytes];
    if entry_bytes != 0 {
        allocation_lengths.push(entry_bytes);
    }
    if string_bytes != 0 {
        allocation_lengths.push(string_bytes);
    }
    allocation_lengths.extend(bucket_lengths.into_iter().filter(|length| *length != 0));
    let variable_bytes = allocation_lengths
        .iter()
        .try_fold(0_usize, |total, length| {
            total
                .checked_add(*length)
                .ok_or(WriterError::ValueTooLarge("named-property map heap page"))
        })?;
    let total = base_size
        .checked_add(variable_bytes)
        .and_then(|size| {
            allocation_lengths
                .len()
                .checked_mul(2)
                .and_then(|map| size.checked_add(map))
        })
        .ok_or(WriterError::ValueTooLarge("named-property map heap page"))?;
    if total > MAX_DATA_BLOCK_PAYLOAD {
        return Err(WriterError::ValueTooLarge("named-property map heap page"));
    }
    Ok(())
}

fn validate_recipient_table_shape(message: &MessageSpec) -> Result<(), WriterError> {
    let rows = (0..message.recipients.len())
        .map(|index| {
            let index = u32::try_from(index)
                .map_err(|_| WriterError::ValueTooLarge("recipient row count"))?;
            let index = index
                .checked_add(1)
                .ok_or(WriterError::ValueTooLarge("recipient row count"))?;
            Ok(TableRowSpec {
                id: NodeId::from(index),
                values: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, WriterError>>()?;
    let base_size = table_context(&recipient_columns()?, &rows)?.len();
    let (variable_bytes, variable_count) =
        message
            .recipients
            .iter()
            .try_fold((0_usize, 0_usize), |(bytes, count), recipient| {
                let mut lengths = [
                    unicode_payload_len(&recipient.display_name)?,
                    8,
                    unicode_payload_len(&recipient.email_address)?,
                    unicode_payload_len(&recipient.email_address)?,
                ];
                lengths
                    .iter_mut()
                    .try_fold((bytes, count), |(bytes, count), length| {
                        if *length == 0 {
                            return Ok::<_, WriterError>((bytes, count));
                        }
                        Ok((
                            bytes
                                .checked_add(*length)
                                .ok_or(WriterError::ValueTooLarge("recipient table heap page"))?,
                            count.checked_add(1).ok_or(WriterError::ValueTooLarge(
                                "recipient table allocation count",
                            ))?,
                        ))
                    })
            })?;
    let total = base_size
        .checked_add(variable_bytes)
        .and_then(|size| {
            variable_count
                .checked_mul(2)
                .and_then(|map| size.checked_add(map))
        })
        .ok_or(WriterError::ValueTooLarge("recipient table heap page"))?;
    if total > MAX_DATA_BLOCK_PAYLOAD {
        return Err(WriterError::ValueTooLarge("recipient table heap page"));
    }
    Ok(())
}

fn validate_attachment_table_shape(message: &MessageSpec) -> Result<(), WriterError> {
    let rows = (0..message.attachments.len())
        .map(|index| {
            let index =
                u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("attachment count"))?;
            Ok(TableRowSpec {
                id: node(
                    NodeIdType::Attachment,
                    0x2_0000_u32
                        .checked_add(index)
                        .ok_or(WriterError::ValueTooLarge("attachment node"))?,
                )?,
                values: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, WriterError>>()?;
    let base_size = table_context(&attachment_columns()?, &rows)?.len();
    let variable_bytes = message
        .attachments
        .iter()
        .try_fold(0_usize, |total, attachment| {
            total
                .checked_add(unicode_payload_len(&attachment.filename)?)
                .ok_or(WriterError::ValueTooLarge("attachment table heap page"))
        })?;
    let total = base_size
        .checked_add(variable_bytes)
        .and_then(|size| {
            message
                .attachments
                .len()
                .checked_mul(2)
                .and_then(|map| size.checked_add(map))
        })
        .ok_or(WriterError::ValueTooLarge("attachment table heap page"))?;
    if total > MAX_DATA_BLOCK_PAYLOAD {
        return Err(WriterError::ValueTooLarge("attachment table heap page"));
    }
    Ok(())
}

fn validate_attachment_property_context_shape(
    attachment: &AttachmentSpec,
) -> Result<(), WriterError> {
    let mut property_count = 8_usize;
    let filename_bytes = unicode_payload_len(&attachment.filename)?;
    let mut lengths = vec![filename_bytes, filename_bytes];
    for value in [
        &attachment.mime_type,
        &attachment.content_id,
        &attachment.content_location,
    ]
    .into_iter()
    .flatten()
    {
        property_count = property_count
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("attachment property count"))?;
        lengths.push(unicode_payload_len(value)?);
    }
    property_count = property_count
        .checked_add(attachment.raw_properties.len())
        .ok_or(WriterError::ValueTooLarge("attachment property count"))?;
    for property in &attachment.raw_properties {
        lengths.push(raw_value_payload_len(&property.value)?);
    }
    match &attachment.content {
        AttachmentContent::Binary(data) if data.len() <= 2048 => lengths.push(data.len()),
        AttachmentContent::Embedded(_) => lengths.push(8),
        AttachmentContent::Binary(_) | AttachmentContent::Spooled(_) => {}
    }
    validate_property_context_shape("attachment property context", property_count, &lengths)
}

fn validate_property_context_shape(
    name: &'static str,
    property_count: usize,
    variable_lengths: &[usize],
) -> Result<(), WriterError> {
    let placeholders = (0..property_count)
        .map(|index| {
            Ok((
                u16::try_from(index).map_err(|_| WriterError::ValueTooLarge("property count"))?,
                PropertyValue::Integer32(0),
            ))
        })
        .collect::<Result<Vec<_>, WriterError>>()?;
    let base_size = property_context(&placeholders)
        .map_err(|error| match error {
            WriterError::ValueTooLarge(_) => WriterError::ValueTooLarge(name),
            other => other,
        })?
        .len();
    let (variable_bytes, allocation_count) =
        variable_lengths
            .iter()
            .try_fold((0_usize, 0_usize), |(bytes, count), length| {
                if *length == 0 || *length > 2048 {
                    return Ok::<_, WriterError>((bytes, count));
                }
                Ok((
                    bytes
                        .checked_add(*length)
                        .ok_or(WriterError::ValueTooLarge(name))?,
                    count
                        .checked_add(1)
                        .ok_or(WriterError::ValueTooLarge(name))?,
                ))
            })?;
    let aligned_bytes = variable_bytes
        .checked_add(variable_bytes % 2)
        .ok_or(WriterError::ValueTooLarge(name))?;
    let total = base_size
        .checked_add(aligned_bytes)
        .and_then(|size| {
            allocation_count
                .checked_mul(2)
                .and_then(|map| size.checked_add(map))
        })
        .ok_or(WriterError::ValueTooLarge(name))?;
    if total > MAX_DATA_BLOCK_PAYLOAD {
        return Err(WriterError::ValueTooLarge(name));
    }
    Ok(())
}

fn validate_message_property_context_shape(message: &MessageSpec) -> Result<(), WriterError> {
    let has_sender = !message.sender_name.is_empty() && !message.sender_email.is_empty();
    let mut property_count = if has_sender { 18_usize } else { 12_usize };
    let mut lengths = vec![
        unicode_payload_len(&message.message_class)?,
        unicode_payload_len(&message.subject)?,
        16,
        8,
        8,
        8,
        8,
    ];
    if has_sender {
        lengths.extend([
            unicode_payload_len(&message.sender_name)?,
            8,
            unicode_payload_len(&message.sender_email)?,
            unicode_payload_len(&message.sender_name)?,
            8,
            unicode_payload_len(&message.sender_email)?,
        ]);
    }
    if let Some(body) = &message.body_text {
        property_count += 1;
        lengths.push(unicode_payload_len(body)?);
    }
    if let Some(body) = &message.body_html {
        property_count += 1;
        lengths.push(body.len());
    }
    if let Some(body) = &message.body_rtf {
        property_count += 2;
        lengths.push(rtf_container_len(body.len())?);
    } else if message
        .spooled_properties
        .iter()
        .any(|property| property.id == 0x1009)
    {
        property_count += 1;
    }
    if message.native_body.is_some() {
        property_count += 1;
    }
    if let Some(headers) = &message.internet_headers {
        property_count += 1;
        lengths.push(unicode_payload_len(headers)?);
    }
    for kind in [RecipientKind::To, RecipientKind::Cc, RecipientKind::Bcc] {
        let recipients = message
            .recipients
            .iter()
            .filter(|recipient| recipient.kind == kind)
            .collect::<Vec<_>>();
        if recipients.is_empty() {
            continue;
        }
        property_count += 1;
        let display_bytes =
            recipients
                .iter()
                .enumerate()
                .try_fold(0_usize, |total, (index, recipient)| {
                    let display_size = unicode_payload_len(&recipient.display_name)?;
                    total
                        .checked_add(if index == 0 { 0 } else { 4 })
                        .and_then(|total| total.checked_add(display_size))
                        .ok_or(WriterError::ValueTooLarge("display recipient property"))
                })?;
        lengths.push(display_bytes);
    }
    property_count = property_count
        .checked_add(message.named_properties.len())
        .and_then(|count| count.checked_add(message.raw_properties.len()))
        .and_then(|count| count.checked_add(message.spooled_properties.len()))
        .ok_or(WriterError::ValueTooLarge("message property count"))?;
    for value in message
        .named_properties
        .iter()
        .map(|property| &property.value)
        .chain(
            message
                .raw_properties
                .iter()
                .map(|property| &property.value),
        )
    {
        lengths.push(raw_value_payload_len(value)?);
    }
    validate_property_context_shape("message property context", property_count, &lengths)
}

fn validate_message(message: &MessageSpec, depth: usize) -> Result<(), WriterError> {
    if !supported_message_class(&message.message_class) {
        return Err(WriterError::InvalidStructure(format!(
            "unsupported message class: {}",
            message.message_class
        )));
    }
    if depth == 0 && calendar_exception_message_class(&message.message_class) {
        return Err(WriterError::InvalidStructure(
            "calendar-exception messages must be embedded".to_owned(),
        ));
    }
    if message.internet_codepage <= 0 {
        return Err(WriterError::InvalidStructure(
            "Internet codepage must be positive".to_owned(),
        ));
    }
    for (name, value) in [
        ("message class", &message.message_class),
        ("subject", &message.subject),
    ] {
        if value.is_empty() {
            return Err(WriterError::InvalidStructure(format!(
                "{name} must be non-empty"
            )));
        }
        validate_unicode(name, value)?;
    }
    for (name, value) in [
        ("sender name", &message.sender_name),
        ("sender email", &message.sender_email),
    ] {
        if value.is_empty() && !sender_optional_message_class(&message.message_class) {
            return Err(WriterError::InvalidStructure(format!(
                "{name} must be non-empty"
            )));
        }
        validate_unicode(name, value)?;
    }
    for recipient in &message.recipients {
        if recipient.display_name.is_empty() || recipient.email_address.is_empty() {
            return Err(WriterError::InvalidStructure(
                "recipient display name and email address must be non-empty".to_owned(),
            ));
        }
        validate_unicode("recipient display name", &recipient.display_name)?;
        validate_unicode("recipient email address", &recipient.email_address)?;
    }
    let has_streamed = |id| {
        message
            .spooled_properties
            .iter()
            .any(|property| property.id == id)
    };
    if message.body_rtf.is_none() && !has_streamed(0x1009) && message.rtf_in_sync {
        return Err(WriterError::InvalidStructure(
            "RTF cannot be marked synchronized when no RTF body is present".to_owned(),
        ));
    }
    match message.native_body {
        Some(NativeBody::PlainText) if message.body_text.is_none() && !has_streamed(0x1000) => {
            return Err(WriterError::InvalidStructure(
                "native plain-text body is not present".to_owned(),
            ));
        }
        Some(NativeBody::Rtf) if message.body_rtf.is_none() && !has_streamed(0x1009) => {
            return Err(WriterError::InvalidStructure(
                "native RTF body is not present".to_owned(),
            ));
        }
        Some(NativeBody::Html) if message.body_html.is_none() && !has_streamed(0x1013) => {
            return Err(WriterError::InvalidStructure(
                "native HTML body is not present".to_owned(),
            ));
        }
        _ => {}
    }
    if let Some(body) = &message.body_text {
        if body.is_empty() {
            return Err(WriterError::InvalidStructure(
                "plain-text body must be non-empty when present".to_owned(),
            ));
        }
        validate_payload_len("plain-text body", unicode_payload_len(body)?)?;
    }
    if let Some(body) = &message.body_html {
        if body.is_empty() {
            return Err(WriterError::InvalidStructure(
                "HTML body must be non-empty when present".to_owned(),
            ));
        }
        if std::str::from_utf8(body).is_err() {
            return Err(WriterError::InvalidStructure(
                "HTML body must be valid UTF-8".to_owned(),
            ));
        }
        validate_payload_len("HTML body", body.len())?;
    }
    if let Some(body) = &message.body_rtf {
        validate_payload_len("RTF body", rtf_container_len(body.len())?)?;
    }
    if let Some(headers) = &message.internet_headers {
        if headers.is_empty() {
            return Err(WriterError::InvalidStructure(
                "Internet headers must be non-empty when present".to_owned(),
            ));
        }
        validate_payload_len("Internet headers", unicode_payload_len(headers)?)?;
    }
    let mut streamed_ids = BTreeSet::new();
    for property in &message.spooled_properties {
        let property_type = PropertyType::try_from(property.property_type).map_err(|_| {
            WriterError::InvalidStructure(format!(
                "unsupported streamed property type: 0x{:04X}",
                property.property_type
            ))
        })?;
        if matches!(
            property_type,
            PropertyType::Null
                | PropertyType::Integer16
                | PropertyType::Integer32
                | PropertyType::Floating32
                | PropertyType::ErrorCode
                | PropertyType::Boolean
                | PropertyType::Object
        ) {
            return Err(WriterError::InvalidStructure(
                "streamed property type must use external storage".to_owned(),
            ));
        }
        if property.id >= 0x8000
            || !streamed_ids.insert(property.id)
            || message
                .raw_properties
                .iter()
                .any(|raw| raw.id == property.id)
        {
            return Err(WriterError::InvalidStructure(
                "streamed property identifier is duplicated or reserved".to_owned(),
            ));
        }
        let allowed_managed = match property.id {
            0x007D => {
                message.internet_headers.is_none()
                    && matches!(property_type, PropertyType::String8 | PropertyType::Unicode)
            }
            0x1000 => {
                message.body_text.is_none()
                    && matches!(property_type, PropertyType::String8 | PropertyType::Unicode)
            }
            0x1009 => message.body_rtf.is_none() && property_type == PropertyType::Binary,
            0x1013 => message.body_html.is_none() && property_type == PropertyType::Binary,
            _ => false,
        };
        if explicit_message_property(property.id) && !allowed_managed {
            return Err(WriterError::InvalidStructure(
                "streamed property collides with a writer-managed property".to_owned(),
            ));
        }
        if property.blob.path.as_os_str().is_empty() || property.blob.byte_len == 0 {
            return Err(WriterError::InvalidStructure(
                "streamed property blob must be non-empty".to_owned(),
            ));
        }
        if property.blob.byte_len > i32::MAX as u64 {
            return Err(WriterError::ValueTooLarge("streamed property blob"));
        }
    }
    for attachment in &message.attachments {
        if attachment.filename.is_empty() {
            return Err(WriterError::InvalidStructure(
                "attachment filename must be non-empty".to_owned(),
            ));
        }
        validate_unicode("attachment filename", &attachment.filename)?;
        if let Some(mime_type) = &attachment.mime_type {
            if mime_type.is_empty() {
                return Err(WriterError::InvalidStructure(
                    "attachment MIME type must be non-empty when present".to_owned(),
                ));
            }
            validate_unicode("attachment MIME type", mime_type)?;
        }
        if let Some(content_id) = &attachment.content_id {
            if content_id.is_empty() {
                return Err(WriterError::InvalidStructure(
                    "attachment content ID must be non-empty when present".to_owned(),
                ));
            }
            validate_unicode("attachment content ID", content_id)?;
        }
        if let Some(content_location) = &attachment.content_location {
            if content_location.is_empty() {
                return Err(WriterError::InvalidStructure(
                    "attachment content location must be non-empty when present".to_owned(),
                ));
            }
            validate_unicode("attachment content location", content_location)?;
        }
        let embedded_calendar_exception = matches!(
            &attachment.content,
            AttachmentContent::Embedded(child)
                if calendar_exception_message_class(&child.message_class)
        );
        if embedded_calendar_exception
            && (!appointment_message_class(&message.message_class)
                || !calendar_exception_attachment_has_linkage(attachment))
        {
            return Err(WriterError::InvalidStructure(
                "calendar-exception messages require an appointment parent and linkage properties"
                    .to_owned(),
            ));
        }
        if !attachment.raw_properties.is_empty()
            && (!appointment_message_class(&message.message_class) || !embedded_calendar_exception)
        {
            return Err(WriterError::InvalidStructure(
                "calendar-exception attachment properties require an embedded exception".to_owned(),
            ));
        }
        if attachment.raw_properties.len() > MAX_FIDELITY_COLLECTION_ITEMS {
            return Err(WriterError::ValueTooLarge("attachment raw-property count"));
        }
        let mut raw_ids = attachment
            .raw_properties
            .iter()
            .map(|property| property.id)
            .collect::<Vec<_>>();
        raw_ids.sort_unstable();
        if raw_ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(WriterError::InvalidStructure(
                "duplicate attachment raw property identifier".to_owned(),
            ));
        }
        let raw_bytes =
            attachment
                .raw_properties
                .iter()
                .try_fold(0_usize, |total, property| {
                    if property.id == 0
                        || property.id >= 0x8000
                        || explicit_attachment_property(property.id)
                        || !calendar_exception_attachment_property(property.id)
                    {
                        return Err(WriterError::InvalidStructure(format!(
                            "attachment raw property 0x{:04X} is not a supported calendar-exception property",
                            property.id
                        )));
                    }
                    if !calendar_exception_attachment_property_type_is_valid(
                        property.id,
                        &property.value,
                    ) {
                        return Err(WriterError::InvalidStructure(format!(
                            "attachment raw property 0x{:04X} has the wrong calendar-exception type",
                            property.id
                        )));
                    }
                    validate_raw_value(&property.value)?;
                    total.checked_add(raw_value_payload_len(&property.value)?).ok_or(
                        WriterError::ValueTooLarge("aggregate attachment raw-property payload"),
                    )
                })?;
        if raw_bytes > MAX_FIDELITY_CUSTOM_PROPERTY_BYTES {
            return Err(WriterError::ValueTooLarge(
                "aggregate attachment raw-property payload",
            ));
        }
        if let AttachmentContent::Binary(data) = &attachment.content {
            validate_payload_len("attachment payload", data.len())?;
        }
        if let AttachmentContent::Spooled(blob) = &attachment.content {
            if blob.path.as_os_str().is_empty() || blob.byte_len == 0 {
                return Err(WriterError::InvalidStructure(
                    "spooled attachment payload must be non-empty".to_owned(),
                ));
            }
            if blob.byte_len > i32::MAX as u64 {
                return Err(WriterError::ValueTooLarge("spooled attachment payload"));
            }
        }
        if let AttachmentContent::Embedded(child) = &attachment.content {
            let child_depth = depth
                .checked_add(1)
                .ok_or(WriterError::ValueTooLarge("embedded message depth"))?;
            if child_depth > MAX_EMBEDDED_MESSAGE_DEPTH {
                return Err(WriterError::ValueTooLarge("embedded message depth"));
            }
            validate_message(child, child_depth)?;
        }
    }
    if message.named_properties.len() > MAX_FIDELITY_COLLECTION_ITEMS {
        return Err(WriterError::ValueTooLarge("named-property count"));
    }
    let mut named_keys = message
        .named_properties
        .iter()
        .map(|property| (property.set, &property.name))
        .collect::<Vec<_>>();
    named_keys.sort();
    if named_keys.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(WriterError::InvalidStructure(
            "duplicate named-property identity".to_owned(),
        ));
    }
    for property in &message.named_properties {
        if let NamedPropertyName::String(name) = &property.name {
            validate_unicode("named-property name", name)?;
        }
        validate_raw_value(&property.value)?;
    }
    if message.raw_properties.len() > MAX_FIDELITY_COLLECTION_ITEMS {
        return Err(WriterError::ValueTooLarge("raw-property count"));
    }
    let custom_property_bytes = message
        .named_properties
        .iter()
        .map(|property| &property.value)
        .chain(
            message
                .raw_properties
                .iter()
                .map(|property| &property.value),
        )
        .try_fold(0_usize, |total, value| {
            total
                .checked_add(raw_value_payload_len(value)?)
                .ok_or(WriterError::ValueTooLarge(
                    "aggregate custom-property payload",
                ))
        })?;
    if custom_property_bytes > MAX_FIDELITY_CUSTOM_PROPERTY_BYTES {
        return Err(WriterError::ValueTooLarge(
            "aggregate custom-property payload",
        ));
    }
    let mut raw_ids = message
        .raw_properties
        .iter()
        .map(|property| property.id)
        .collect::<Vec<_>>();
    raw_ids.sort_unstable();
    if raw_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(WriterError::InvalidStructure(
            "duplicate raw property identifier".to_owned(),
        ));
    }
    for property in &message.raw_properties {
        if property.id == 0 || property.id >= 0x8000 || explicit_message_property(property.id) {
            return Err(WriterError::InvalidStructure(format!(
                "raw property 0x{:04X} conflicts with writer-managed properties",
                property.id
            )));
        }
        validate_raw_value(&property.value)?;
    }
    if depth == 0 {
        validate_contents_raw_property_types(message)?;
    }
    validate_message_property_context_shape(message)?;
    Ok(())
}

fn validate_contents_raw_property_types(message: &MessageSpec) -> Result<(), WriterError> {
    let columns = contents_columns()?;
    for raw in &message.raw_properties {
        let Some(column) = columns.iter().find(|column| column.prop_id() == raw.id) else {
            continue;
        };
        let actual = raw_property_value(&raw.value).property_type();
        if actual != column.prop_type() {
            return Err(WriterError::InvalidStructure(format!(
                "raw property 0x{:04X} has type {actual:?}, expected {:?} for the contents table",
                raw.id,
                column.prop_type()
            )));
        }
    }
    Ok(())
}

fn supported_message_class(value: &str) -> bool {
    !value.is_empty()
}

fn contact_message_class(value: &str) -> bool {
    class_is_or_descends_from(value, "IPM.Contact")
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

fn validate_unicode(name: &'static str, value: &str) -> Result<(), WriterError> {
    if value.encode_utf16().count() > 2048 {
        return Err(WriterError::ValueTooLarge(name));
    }
    Ok(())
}

fn validate_raw_value(value: &RawPropertyValue) -> Result<(), WriterError> {
    if matches!(value, RawPropertyValue::MultipleInteger16(value) if value.is_empty())
        || matches!(value, RawPropertyValue::MultipleInteger32(value) if value.is_empty())
        || matches!(value, RawPropertyValue::MultipleInteger64(value) if value.is_empty())
    {
        return Err(WriterError::InvalidStructure(
            "typed variable raw properties must be non-empty".to_owned(),
        ));
    }
    validate_payload_len("raw property", raw_value_payload_len(value)?)
}

fn raw_value_payload_len(value: &RawPropertyValue) -> Result<usize, WriterError> {
    let encoded_len = match value {
        RawPropertyValue::Integer16(_)
        | RawPropertyValue::Integer32(_)
        | RawPropertyValue::Floating32(_)
        | RawPropertyValue::ErrorCode(_)
        | RawPropertyValue::Boolean(_) => 0,
        RawPropertyValue::Integer64(_)
        | RawPropertyValue::Floating64(_)
        | RawPropertyValue::Currency(_)
        | RawPropertyValue::FloatingTime(_)
        | RawPropertyValue::Time(_) => 8,
        RawPropertyValue::Guid(_) => 16,
        RawPropertyValue::Unicode(value) => unicode_payload_len(value)?,
        RawPropertyValue::Binary(value) => value.len(),
        RawPropertyValue::MultipleInteger16(values) => values
            .len()
            .checked_mul(2)
            .ok_or(WriterError::ValueTooLarge("multi-valued property"))?,
        RawPropertyValue::MultipleInteger32(values) => values
            .len()
            .checked_mul(4)
            .ok_or(WriterError::ValueTooLarge("multi-valued property"))?,
        RawPropertyValue::MultipleInteger64(values) => values
            .len()
            .checked_mul(8)
            .ok_or(WriterError::ValueTooLarge("multi-valued property"))?,
        RawPropertyValue::MultipleGuid(values) => values
            .len()
            .checked_mul(16)
            .ok_or(WriterError::ValueTooLarge("multi-valued property"))?,
    };
    Ok(encoded_len)
}

fn unicode_payload_len(value: &str) -> Result<usize, WriterError> {
    value
        .encode_utf16()
        .count()
        .checked_mul(2)
        .ok_or(WriterError::ValueTooLarge("Unicode property"))
}

fn validate_payload_len(name: &'static str, length: usize) -> Result<(), WriterError> {
    if length > MAX_FIDELITY_PROPERTY_BYTES {
        return Err(WriterError::ValueTooLarge(name));
    }
    Ok(())
}

fn explicit_message_property(id: u16) -> bool {
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

fn explicit_attachment_property(id: u16) -> bool {
    matches!(
        id,
        0x0E20
            | 0x0E21
            | 0x3701
            | 0x3704
            | 0x3705
            | 0x3707
            | 0x370B
            | 0x370E
            | 0x3712
            | 0x3713
            | 0x3714
    )
}

fn calendar_exception_attachment_property(id: u16) -> bool {
    matches!(id, 0x3001 | 0x3702 | 0x3709 | 0x7FFA..=0x7FFF)
}

fn calendar_exception_attachment_property_type_is_valid(id: u16, value: &RawPropertyValue) -> bool {
    matches!(
        (id, value),
        (0x3001, RawPropertyValue::Unicode(_))
            | (0x3702 | 0x3709, RawPropertyValue::Binary(_))
            | (0x7FFA | 0x7FFD, RawPropertyValue::Integer32(_))
            | (0x7FFB | 0x7FFC, RawPropertyValue::Time(_))
            | (0x7FFE | 0x7FFF, RawPropertyValue::Boolean(_))
    )
}

fn calendar_exception_attachment_has_linkage(attachment: &AttachmentSpec) -> bool {
    (0x7FFA..=0x7FFE).all(|id| {
        attachment
            .raw_properties
            .iter()
            .any(|property| property.id == id)
    })
}

fn validation_property_ids(
    message: &MessageSpec,
    named_identities: &[NamedIdentity],
) -> Result<Vec<u16>, WriterError> {
    let mut ids = vec![
        0x001A, 0x0037, 0x0039, 0x0042, 0x0064, 0x0065, 0x007D, 0x0C1A, 0x0C1E, 0x0C1F, 0x0E02,
        0x0E03, 0x0E04, 0x0E06, 0x0E07, 0x0E08, 0x0E17, 0x0E1B, 0x0E1F, 0x1000, 0x1009, 0x1013,
        0x1016, 0x3007, 0x3008, 0x300B, 0x3FDE,
    ];
    ids.extend(message.raw_properties.iter().map(|property| property.id));
    for property in &message.named_properties {
        let index = named_identities
            .binary_search(&(property.set, property.name.clone()))
            .map_err(|_| {
                WriterError::InvalidStructure("named property is not mapped".to_owned())
            })?;
        ids.push(
            0x8000_u16
                .checked_add(
                    u16::try_from(index)
                        .map_err(|_| WriterError::ValueTooLarge("named-property count"))?,
                )
                .ok_or(WriterError::ValueTooLarge("named-property identifier"))?,
        );
    }
    let streamed = message
        .spooled_properties
        .iter()
        .map(|property| property.id)
        .collect::<BTreeSet<_>>();
    ids.retain(|id| !streamed.contains(id));
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

fn validate_completed_store(
    path: &Path,
    spec: &StoreInput<'_>,
    mail: NodeId,
    named_identities: &[NamedIdentity],
) -> Result<(), WriterError> {
    use crate::{ltp::prop_context::PropertyValue as ReadValue, messaging::store::EntryId};

    let invalid = |message: &str| WriterError::InvalidStructure(message.to_owned());
    let store = crate::open_store(path)?;
    if store.properties().display_name()? != spec.store_name {
        return Err(invalid("completed store display name mismatch"));
    }

    let message = node(NodeIdType::NormalMessage, MESSAGE_INDEX)?;
    let mail_entry = store.properties().make_entry_id(mail)?;
    let folder = store.open_folder(&mail_entry)?;
    let contents = folder
        .contents_table()
        .ok_or_else(|| invalid("completed store mail contents table is missing"))?;
    let row = contents
        .find_row(crate::ltp::table_context::TableRowId::new(u32::from(
            message,
        )))
        .map_err(|_| invalid("completed store message is not indexed"))?;
    let values = row.columns(contents.context())?;
    let table_value = |property_id| -> Result<Option<ReadValue>, WriterError> {
        let column = contents
            .context()
            .columns()
            .iter()
            .position(|column| column.prop_id() == property_id)
            .ok_or_else(|| invalid("completed store contents column is missing"))?;
        let Some(value) = values[column].as_ref() else {
            return Ok(None);
        };
        Ok(Some(contents.read_column(
            value,
            contents.context().columns()[column].prop_type(),
        )?))
    };
    let size_column = contents
        .context()
        .columns()
        .iter()
        .position(|column| column.prop_id() == 0x0E08)
        .ok_or_else(|| invalid("completed store message-size column is missing"))?;
    let row_size = match values.get(size_column) {
        Some(Some(crate::ltp::table_context::TableRowColumnValue::Small(
            ReadValue::Integer32(value),
        ))) => *value,
        _ => return Err(invalid("completed store message-size row value is invalid")),
    };
    let received_column = contents
        .context()
        .columns()
        .iter()
        .position(|column| column.prop_id() == 0x0E06)
        .ok_or_else(|| invalid("completed store delivery-time column is missing"))?;
    if !matches!(
        values.get(received_column),
        Some(Some(crate::ltp::table_context::TableRowColumnValue::Small(
            ReadValue::Time(value),
        ))) if *value == spec.message.received_filetime
    ) {
        return Err(invalid(
            "completed store delivery-time row value is invalid",
        ));
    }
    let sender_row_matches = if spec.message.sender_name.is_empty() {
        table_value(0x0042)?.is_none()
    } else {
        matches!(table_value(0x0042)?, Some(ReadValue::Unicode(value)) if value.to_string() == spec.message.sender_name)
    };
    if !matches!(table_value(0x0039)?, Some(ReadValue::Time(value)) if value == spec.message.sent_filetime)
        || !sender_row_matches
        || !matches!(table_value(0x0E17)?, Some(ReadValue::Integer32(0)))
    {
        return Err(invalid("completed store copied contents value mismatch"));
    }
    let expected_display = display_recipient_properties(&spec.message.recipients);
    for id in [0x0E03, 0x0E04] {
        let expected = expected_display
            .iter()
            .find_map(|(property_id, value)| (*property_id == id).then_some(value));
        match (expected, table_value(id)?) {
            (Some(PropertyValue::Unicode(expected)), Some(ReadValue::Unicode(actual)))
                if actual.to_string() == *expected => {}
            (None, None) => {}
            _ => return Err(invalid("completed store copied display-recipient mismatch")),
        }
    }

    let validation_ids = validation_property_ids(spec.message, named_identities)?;
    let message_entry = EntryId::new(
        crate::messaging::store::StoreRecordKey::new(spec.record_key),
        message,
    );
    let message = store.open_message(&message_entry, Some(&validation_ids))?;
    if message.properties().message_class()? != spec.message.message_class {
        return Err(invalid("completed store message class mismatch"));
    }
    if message.properties().message_size()? != row_size {
        return Err(invalid("completed store message-size values disagree"));
    }
    for (property, expected, name) in [(0x0037, &spec.message.subject, "subject")] {
        match message.properties().get(property) {
            Some(ReadValue::Unicode(value)) if value.to_string() == *expected => {}
            _ => return Err(invalid(&format!("completed store {name} mismatch"))),
        }
    }
    if spec.message.sender_name.is_empty() && spec.message.sender_email.is_empty() {
        for property in [0x0042, 0x0064, 0x0065, 0x0C1A, 0x0C1E, 0x0C1F] {
            if message.properties().get(property).is_some() {
                return Err(invalid("completed store unexpected sender property"));
            }
        }
    } else {
        for (property, expected, name) in [
            (0x0042, &spec.message.sender_name, "sender name"),
            (0x0065, &spec.message.sender_email, "sender email"),
            (0x0C1A, &spec.message.sender_name, "sender duplicate name"),
            (0x0C1F, &spec.message.sender_email, "sender duplicate email"),
        ] {
            match message.properties().get(property) {
                Some(ReadValue::Unicode(value)) if value.to_string() == *expected => {}
                _ => return Err(invalid(&format!("completed store {name} mismatch"))),
            }
        }
        for property in [0x0064, 0x0C1E] {
            if !matches!(message.properties().get(property), Some(ReadValue::Unicode(value)) if value.to_string() == "SMTP")
            {
                return Err(invalid("completed store sender address type mismatch"));
            }
        }
    }
    let expected_flags = output_message_flags(spec.message, spec.associated);
    if !matches!(message.properties().get(0x0E07), Some(ReadValue::Integer32(value)) if *value == expected_flags)
        || !matches!(message.properties().get(0x0E1B), Some(ReadValue::Boolean(value)) if *value != spec.message.attachments.is_empty())
        || !matches!(
            message.properties().get(0x3FDE),
            Some(ReadValue::Integer32(value)) if *value == spec.message.internet_codepage
        )
    {
        return Err(invalid("completed store attachment flags mismatch"));
    }
    match (&spec.message.body_text, message.properties().get(0x1000)) {
        (Some(expected), Some(ReadValue::Unicode(actual))) if actual.to_string() == *expected => {}
        (None, None) => {}
        _ => return Err(invalid("completed store plain body mismatch")),
    }
    match (&spec.message.body_html, message.properties().get(0x1013)) {
        (Some(expected), Some(ReadValue::Binary(actual))) if actual.buffer() == expected => {}
        (None, None) => {}
        _ => return Err(invalid("completed store HTML body mismatch")),
    }
    match (&spec.message.body_rtf, message.properties().get(0x1009)) {
        (Some(expected), Some(ReadValue::Binary(actual)))
            if actual.buffer() == rtf_container(expected)? => {}
        (None, None) => {}
        _ => return Err(invalid("completed store RTF body mismatch")),
    }
    let has_streamed_rtf = spec
        .message
        .spooled_properties
        .iter()
        .any(|property| property.id == 0x1009);
    match (
        spec.message.body_rtf.is_some() || has_streamed_rtf,
        message.properties().get(0x0E1F),
    ) {
        (true, Some(ReadValue::Boolean(actual))) if *actual == spec.message.rtf_in_sync => {}
        (false, None) => {}
        _ => return Err(invalid("completed store RTF synchronization flag mismatch")),
    }
    match (spec.message.native_body, message.properties().get(0x1016)) {
        (Some(expected), Some(ReadValue::Integer32(actual))) if *actual == expected as i32 => {}
        (None, None) => {}
        _ => return Err(invalid("completed store native body mismatch")),
    }
    match (
        &spec.message.internet_headers,
        message.properties().get(0x007D),
    ) {
        (Some(expected), Some(ReadValue::Unicode(actual))) if actual.to_string() == *expected => {}
        (None, None) => {}
        _ => return Err(invalid("completed store Internet headers mismatch")),
    }
    if !matches!(
        message.properties().get(0x0039),
        Some(ReadValue::Time(value)) if *value == spec.message.sent_filetime
    ) || !matches!(
        message.properties().get(0x0E06),
        Some(ReadValue::Time(value)) if *value == spec.message.received_filetime
    ) || !matches!(
        message.properties().get(0x3007),
        Some(ReadValue::Time(value)) if *value == spec.message.creation_filetime
    ) || !matches!(
        message.properties().get(0x3008),
        Some(ReadValue::Time(value)) if *value == spec.message.modification_filetime
    ) {
        return Err(invalid("completed store timestamps mismatch"));
    }
    let expected_record_key = message_record_key(spec.record_key, message_entry.node_id());
    if !matches!(
        message.properties().get(0x300B),
        Some(ReadValue::Binary(value)) if value.buffer() == expected_record_key
    ) {
        return Err(invalid("completed store message record key mismatch"));
    }
    let expected_display = display_recipient_properties(&spec.message.recipients);
    for id in [0x0E02, 0x0E03, 0x0E04] {
        let expected = expected_display
            .iter()
            .find_map(|(property_id, value)| (*property_id == id).then_some(value));
        match (expected, message.properties().get(id)) {
            (Some(PropertyValue::Unicode(expected)), Some(ReadValue::Unicode(actual)))
                if actual.to_string() == *expected => {}
            (None, None) => {}
            _ => return Err(invalid("completed store display-recipient mismatch")),
        }
    }
    for property in &spec.message.named_properties {
        let index = named_identities
            .binary_search(&(property.set, property.name.clone()))
            .map_err(|_| invalid("completed store named property is not mapped"))?;
        let id = 0x8000_u16
            .checked_add(
                u16::try_from(index)
                    .map_err(|_| invalid("completed store named property index overflow"))?,
            )
            .ok_or_else(|| invalid("completed store named property ID overflow"))?;
        if !message
            .properties()
            .get(id)
            .is_some_and(|actual| raw_value_matches(&property.value, actual))
        {
            return Err(invalid(&format!(
                "completed store named property 0x{id:04X} mismatch"
            )));
        }
    }
    for property in &spec.message.raw_properties {
        if !message
            .properties()
            .get(property.id)
            .is_some_and(|actual| raw_value_matches(&property.value, actual))
        {
            return Err(invalid("completed store raw property mismatch"));
        }
    }
    let recipients = message
        .recipient_table()
        .ok_or_else(|| invalid("completed store recipient table is missing"))?;
    if recipients
        .find_row(crate::ltp::table_context::TableRowId::new(0))
        .is_ok()
    {
        return Err(invalid(
            "completed store recipient table contains row ID zero",
        ));
    }
    if recipients.rows_matrix().count() != spec.message.recipients.len() {
        return Err(invalid("completed store recipient count mismatch"));
    }
    let recipient_columns = recipients.context().columns();
    let column_index = |property_id| {
        recipient_columns
            .iter()
            .position(|column| column.prop_id() == property_id)
            .ok_or_else(|| invalid("completed store recipient column is missing"))
    };
    let role_column = column_index(0x0C15)?;
    let name_column = column_index(0x3001)?;
    let address_type_column = column_index(0x3002)?;
    let email_column = column_index(0x3003)?;
    let smtp_column = column_index(0x39FF)?;
    for (index, (row, expected)) in recipients
        .rows_matrix()
        .zip(spec.message.recipients.iter())
        .enumerate()
    {
        let expected_row = u32::try_from(index)
            .ok()
            .and_then(|row| row.checked_add(1))
            .ok_or_else(|| invalid("completed store recipient row ID overflow"))?;
        let expected_row = crate::ltp::table_context::TableRowId::new(expected_row);
        let indexed_row = recipients
            .find_row(expected_row)
            .map_err(|_| invalid("completed store recipient row is not indexed"))?;
        if row.id() != expected_row || indexed_row.id() != expected_row {
            return Err(invalid("completed store recipient row ID mismatch"));
        }
        let values = row.columns(recipients.context())?;
        let read = |index: usize| {
            let value = values[index]
                .as_ref()
                .ok_or_else(|| invalid("completed store recipient value is missing"))?;
            recipients
                .read_column(value, recipient_columns[index].prop_type())
                .map_err(WriterError::from)
        };
        if !matches!(read(role_column)?, ReadValue::Integer32(value) if value == expected.kind as i32)
            || !matches!(read(name_column)?, ReadValue::Unicode(value) if value.to_string() == expected.display_name)
            || !matches!(read(address_type_column)?, ReadValue::Unicode(value) if value.to_string() == "SMTP")
            || !matches!(read(email_column)?, ReadValue::Unicode(value) if value.to_string() == expected.email_address)
            || !matches!(read(smtp_column)?, ReadValue::Unicode(value) if value.to_string() == expected.email_address)
        {
            return Err(invalid("completed store recipient value mismatch"));
        }
    }
    let attachments = message
        .attachment_table()
        .ok_or_else(|| invalid("completed store attachment table is missing"))?;
    if attachments.rows_matrix().count() != spec.message.attachments.len() {
        return Err(invalid("completed store attachment count mismatch"));
    }
    for index in 0..spec.message.attachments.len() {
        let index =
            u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("attachment count"))?;
        let attachment = node(
            NodeIdType::Attachment,
            0x2_0000_u32
                .checked_add(index)
                .ok_or(WriterError::ValueTooLarge("attachment node"))?,
        )?;
        attachments
            .find_row(crate::ltp::table_context::TableRowId::new(u32::from(
                attachment,
            )))
            .map_err(|_| invalid("completed store attachment row identity mismatch"))?;
    }
    validate_attachment_fidelity(path, spec, named_identities)?;
    Ok(())
}

fn validate_completed_folder_store(
    path: &Path,
    record_key: [u8; 16],
    folders: &[FolderPlan<'_>],
) -> Result<(), WriterError> {
    let invalid = |message: &str| WriterError::InvalidStructure(message.to_owned());
    let store = crate::open_store(path)?;
    let root_node = NID_ROOT_FOLDER;
    let root = store.open_folder(&store.properties().make_entry_id(root_node)?)?;
    let ipm_node = node(NodeIdType::NormalFolder, IPM_FOLDER_INDEX)?;
    let ipm = store.open_folder(&store.properties().make_entry_id(ipm_node)?)?;
    let mut message_index = 0_u32;
    for folder_plan in folders {
        let actual_parent = if let Some(parent) = folder_plan.parent {
            store.open_folder(&store.properties().make_entry_id(folders[parent].node)?)?
        } else {
            match folder_plan.location {
                MailFolderLocation::StoreRoot => root.clone(),
                MailFolderLocation::IpmSubtree => ipm.clone(),
            }
        };
        actual_parent
            .hierarchy_table()
            .ok_or_else(|| invalid("completed parent hierarchy table is missing"))?
            .find_row(crate::ltp::table_context::TableRowId::new(u32::from(
                folder_plan.node,
            )))
            .map_err(|_| invalid("completed child folder is not indexed"))?;

        let folder = store.open_folder(&store.properties().make_entry_id(folder_plan.node)?)?;
        if folder.properties().display_name()?
            != *folder_plan
                .path
                .last()
                .ok_or_else(|| invalid("planned folder path is empty"))?
            || folder.properties().content_count()?
                != i32::try_from(folder_plan.messages.len())
                    .map_err(|_| WriterError::ValueTooLarge("folder message count"))?
            || folder.properties().unread_count()? != folder_unread_count(&folder_plan.messages)?
            || folder.properties().has_sub_folders()? == folder_plan.children.is_empty()
            || !matches!(
                folder.properties().get(0x3613),
                Some(crate::ltp::prop_context::PropertyValue::Unicode(value))
                    if value.to_string() == folder_plan.container_class
            )
        {
            return Err(invalid("completed folder properties mismatch"));
        }
        let hierarchy = folder
            .hierarchy_table()
            .ok_or_else(|| invalid("completed folder hierarchy table is missing"))?;
        if hierarchy.rows_matrix().count() != folder_plan.children.len() {
            return Err(invalid("completed child folder count mismatch"));
        }
        for child in &folder_plan.children {
            hierarchy
                .find_row(crate::ltp::table_context::TableRowId::new(u32::from(
                    folders[*child].node,
                )))
                .map_err(|_| invalid("completed child hierarchy row is missing"))?;
        }
        let contents = folder
            .contents_table()
            .ok_or_else(|| invalid("completed folder contents table is missing"))?;
        if contents.rows_matrix().count() != folder_plan.messages.len() {
            return Err(invalid("completed folder message count mismatch"));
        }
        for expected in &folder_plan.messages {
            let message = node(
                NodeIdType::NormalMessage,
                MESSAGE_INDEX
                    .checked_add(message_index)
                    .ok_or(WriterError::ValueTooLarge("message node"))?,
            )?;
            message_index = message_index
                .checked_add(1)
                .ok_or(WriterError::ValueTooLarge("message count"))?;
            let row = contents
                .find_row(crate::ltp::table_context::TableRowId::new(u32::from(
                    message,
                )))
                .map_err(|_| invalid("completed store message is not indexed"))?;
            let values = row.columns(contents.context())?;
            let subject = contents
                .context()
                .columns()
                .iter()
                .position(|column| column.prop_id() == 0x0037)
                .ok_or_else(|| invalid("completed store subject column is missing"))?;
            let value = values[subject]
                .as_ref()
                .ok_or_else(|| invalid("completed store subject is missing"))?;
            let actual =
                contents.read_column(value, contents.context().columns()[subject].prop_type())?;
            if !matches!(actual, crate::ltp::prop_context::PropertyValue::Unicode(value) if value.to_string() == expected.subject)
            {
                return Err(invalid("completed store subject mismatch"));
            }
            let flags = contents
                .context()
                .columns()
                .iter()
                .position(|column| column.prop_id() == 0x0E07)
                .ok_or_else(|| invalid("completed store message-flags column is missing"))?;
            let value = values[flags]
                .as_ref()
                .ok_or_else(|| invalid("completed store message flags are missing"))?;
            let actual =
                contents.read_column(value, contents.context().columns()[flags].prop_type())?;
            if !matches!(actual, crate::ltp::prop_context::PropertyValue::Integer32(value) if value == output_message_flags(expected, false))
            {
                return Err(invalid("completed store message flags mismatch"));
            }
            let opened =
                store.open_message(&store.properties().make_entry_id(message)?, Some(&[0x001A]))?;
            if opened.properties().message_class()? != expected.message_class {
                return Err(invalid("completed normal message class mismatch"));
            }
        }
        let associated = folder
            .associated_table()
            .ok_or_else(|| invalid("completed folder associated table is missing"))?;
        if associated.rows_matrix().count() != folder_plan.associated_messages.len() {
            return Err(invalid(
                "completed folder associated message count mismatch",
            ));
        }
        for expected in &folder_plan.associated_messages {
            let message = node(
                NodeIdType::AssociatedMessage,
                MESSAGE_INDEX
                    .checked_add(message_index)
                    .ok_or(WriterError::ValueTooLarge("associated message node"))?,
            )?;
            message_index = message_index
                .checked_add(1)
                .ok_or(WriterError::ValueTooLarge("message count"))?;
            let row = associated
                .find_row(crate::ltp::table_context::TableRowId::new(u32::from(
                    message,
                )))
                .map_err(|_| invalid("completed associated message is not indexed"))?;
            let values = row.columns(associated.context())?;
            let subject = associated
                .context()
                .columns()
                .iter()
                .position(|column| column.prop_id() == 0x3001)
                .ok_or_else(|| invalid("completed associated display-name column is missing"))?;
            let value = values[subject]
                .as_ref()
                .ok_or_else(|| invalid("completed associated display name is missing"))?;
            let actual = associated
                .read_column(value, associated.context().columns()[subject].prop_type())?;
            if !matches!(actual, crate::ltp::prop_context::PropertyValue::Unicode(value) if value.to_string() == associated_display_name(expected))
            {
                return Err(invalid("completed associated display-name mismatch"));
            }
            let flags = associated
                .context()
                .columns()
                .iter()
                .position(|column| column.prop_id() == 0x0E07)
                .ok_or_else(|| invalid("completed associated message-flags column is missing"))?;
            let value = values[flags]
                .as_ref()
                .ok_or_else(|| invalid("completed associated message flags are missing"))?;
            let actual =
                associated.read_column(value, associated.context().columns()[flags].prop_type())?;
            if !matches!(actual, crate::ltp::prop_context::PropertyValue::Integer32(value) if value == output_message_flags(expected, true))
            {
                return Err(invalid("completed associated message-flags mismatch"));
            }
            let opened = store.open_message(
                &store.properties().make_entry_id(message)?,
                Some(&[0x001A, 0x0E07]),
            )?;
            if opened.properties().message_class()? != expected.message_class {
                return Err(invalid("completed associated message class mismatch"));
            }
            if !matches!(opened.properties().get(0x0E07), Some(crate::ltp::prop_context::PropertyValue::Integer32(value)) if *value == output_message_flags(expected, true))
            {
                return Err(invalid(
                    "completed associated message property flags mismatch",
                ));
            }
        }
    }
    validate_spooled_attachment_identities(path, record_key, folders)
}

fn validate_spooled_attachment_identities(
    path: &Path,
    record_key: [u8; 16],
    folders: &[FolderPlan<'_>],
) -> Result<(), WriterError> {
    use crate::{
        UnicodePstFile,
        messaging::{
            attachment::{Attachment, AttachmentData, UnicodeAttachment},
            message::{Message, UnicodeMessage},
            store::{EntryId, UnicodeStore},
        },
    };
    use std::rc::Rc;

    let invalid = |message: &str| WriterError::InvalidStructure(message.to_owned());
    let pst = Rc::new(UnicodePstFile::open(path)?);
    let store = UnicodeStore::read(pst)?;
    let mut message_index = 0_u32;
    for (message_spec, associated) in folders.iter().flat_map(|folder| {
        folder
            .messages
            .iter()
            .map(|message| (message, false))
            .chain(
                folder
                    .associated_messages
                    .iter()
                    .map(|message| (message, true)),
            )
    }) {
        let validate_streamed_attachments = message_index != 0;
        let streamed_ids = message_spec
            .spooled_properties
            .iter()
            .map(|property| property.id)
            .collect::<Vec<_>>();
        let message_node = node(
            if associated {
                NodeIdType::AssociatedMessage
            } else {
                NodeIdType::NormalMessage
            },
            MESSAGE_INDEX
                .checked_add(message_index)
                .ok_or(WriterError::ValueTooLarge("message node"))?,
        )?;
        message_index = message_index
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("message count"))?;
        let message = UnicodeMessage::read_with_streamed_properties(
            store.clone(),
            &EntryId::new(
                crate::messaging::store::StoreRecordKey::new(record_key),
                message_node,
            ),
            Some(&[]),
            &streamed_ids,
        )?;
        for property in &message_spec.spooled_properties {
            if message.streamed_property_identity(property.id)
                != Some((
                    property.property_type,
                    property.blob.byte_len,
                    property.blob.sha256,
                ))
            {
                return Err(invalid(
                    "completed store streamed message property identity mismatch",
                ));
            }
        }
        if !validate_streamed_attachments {
            continue;
        }
        for (attachment_index, attachment_spec) in message_spec.attachments.iter().enumerate() {
            let attachment_index = u32::try_from(attachment_index)
                .map_err(|_| WriterError::ValueTooLarge("attachment count"))?;
            let attachment_node = node(
                NodeIdType::Attachment,
                0x2_0000_u32
                    .checked_add(attachment_index)
                    .ok_or(WriterError::ValueTooLarge("attachment node"))?,
            )?;
            match &attachment_spec.content {
                AttachmentContent::Spooled(expected) => {
                    let attachment =
                        UnicodeAttachment::read_metadata(message.clone(), attachment_node)
                            .map_err(|error| {
                                invalid(&format!(
                                    "completed store streamed attachment cannot be read: {error}"
                                ))
                            })?;
                    if attachment.streamed_data_identity()
                        != Some((expected.byte_len, expected.sha256))
                    {
                        return Err(invalid(
                            "completed store streamed attachment identity mismatch",
                        ));
                    }
                }
                AttachmentContent::Embedded(expected)
                    if !expected.spooled_properties.is_empty() =>
                {
                    let embedded_streamed_ids = expected
                        .spooled_properties
                        .iter()
                        .map(|property| property.id)
                        .collect::<Vec<_>>();
                    let attachment = UnicodeAttachment::read_with_streamed_embedded_properties(
                        message.clone(),
                        attachment_node,
                        Some(&[0x0E08]),
                        &embedded_streamed_ids,
                    )?;
                    let Some(AttachmentData::Message(actual)) = attachment.data() else {
                        return Err(invalid("completed store embedded message is missing"));
                    };
                    for property in &expected.spooled_properties {
                        if actual.streamed_property_identity(property.id)
                            != Some((
                                property.property_type,
                                property.blob.byte_len,
                                property.blob.sha256,
                            ))
                        {
                            return Err(invalid(
                                "completed store embedded streamed property identity mismatch",
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn raw_value_matches(
    expected: &RawPropertyValue,
    actual: &crate::ltp::prop_context::PropertyValue,
) -> bool {
    use crate::ltp::prop_context::PropertyValue as ReadValue;
    match (expected, actual) {
        (RawPropertyValue::Integer16(left), ReadValue::Integer16(right)) => left == right,
        (RawPropertyValue::Integer32(left), ReadValue::Integer32(right)) => left == right,
        (RawPropertyValue::Integer64(left), ReadValue::Integer64(right)) => left == right,
        (RawPropertyValue::Floating32(left), ReadValue::Floating32(right)) => {
            *left == right.to_bits()
        }
        (RawPropertyValue::Floating64(left), ReadValue::Floating64(right)) => {
            *left == right.to_bits()
        }
        (RawPropertyValue::Currency(left), ReadValue::Currency(right)) => left == right,
        (RawPropertyValue::FloatingTime(left), ReadValue::FloatingTime(right)) => {
            *left == right.to_bits()
        }
        (RawPropertyValue::ErrorCode(left), ReadValue::ErrorCode(right)) => {
            left.to_le_bytes() == right.to_le_bytes()
        }
        (RawPropertyValue::Boolean(left), ReadValue::Boolean(right)) => left == right,
        (RawPropertyValue::Time(left), ReadValue::Time(right)) => left == right,
        (RawPropertyValue::Guid(left), ReadValue::Guid(right)) => *left == right.to_le_bytes(),
        (RawPropertyValue::Unicode(left), ReadValue::Unicode(right)) => *left == right.to_string(),
        (RawPropertyValue::Binary(left), ReadValue::Binary(right)) => left == right.buffer(),
        (RawPropertyValue::MultipleInteger16(left), ReadValue::MultipleInteger16(right)) => {
            left == right
        }
        (RawPropertyValue::MultipleInteger32(left), ReadValue::MultipleInteger32(right)) => {
            left == right
        }
        (RawPropertyValue::MultipleInteger64(left), ReadValue::MultipleInteger64(right)) => {
            left == right
        }
        (RawPropertyValue::MultipleGuid(left), ReadValue::MultipleGuid(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| *left == right.to_le_bytes())
        }
        (RawPropertyValue::Unicode(value), ReadValue::Null) => value.is_empty(),
        (RawPropertyValue::Binary(value), ReadValue::Null) => value.is_empty(),
        (RawPropertyValue::MultipleInteger16(value), ReadValue::Null) => value.is_empty(),
        (RawPropertyValue::MultipleInteger32(value), ReadValue::Null) => value.is_empty(),
        (RawPropertyValue::MultipleInteger64(value), ReadValue::Null) => value.is_empty(),
        (RawPropertyValue::MultipleGuid(value), ReadValue::Null) => value.is_empty(),
        _ => false,
    }
}

fn validate_attachment_fidelity(
    path: &Path,
    spec: &StoreInput<'_>,
    named_identities: &[NamedIdentity],
) -> Result<(), WriterError> {
    use crate::{
        UnicodePstFile,
        messaging::{
            attachment::{Attachment, AttachmentData, UnicodeAttachment},
            message::{Message, UnicodeMessage},
            store::{EntryId, Store, UnicodeStore},
        },
    };
    use std::rc::Rc;

    let invalid = |message: &str| WriterError::InvalidStructure(message.to_owned());
    let pst = Rc::new(UnicodePstFile::open(path)?);
    let store = UnicodeStore::read(pst.clone())?;
    let named_map = store.named_property_map()?;
    validate_named_map(named_map.as_ref(), named_identities)?;
    let top_node = node(NodeIdType::NormalMessage, MESSAGE_INDEX)?;
    let top = UnicodeMessage::read(
        store,
        &EntryId::new(
            crate::messaging::store::StoreRecordKey::new(spec.record_key),
            top_node,
        ),
        Some(&[]),
    )?;
    let attachment_table = top
        .attachment_table()
        .ok_or_else(|| invalid("completed store attachment table is missing"))?;
    for (index, expected) in spec.message.attachments.iter().enumerate() {
        let index_u32 =
            u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("attachment count"))?;
        let attachment_node = node(
            NodeIdType::Attachment,
            0x2_0000_u32
                .checked_add(index_u32)
                .ok_or(WriterError::ValueTooLarge("attachment node"))?,
        )?;
        let attachment = match &expected.content {
            AttachmentContent::Spooled(_) => {
                UnicodeAttachment::read_metadata(top.clone(), attachment_node)
            }
            AttachmentContent::Embedded(message) => {
                let validation_ids = validation_property_ids(message, named_identities)?;
                let streamed_ids = message
                    .spooled_properties
                    .iter()
                    .map(|property| property.id)
                    .collect::<Vec<_>>();
                UnicodeAttachment::read_with_streamed_embedded_properties(
                    top.clone(),
                    attachment_node,
                    Some(&validation_ids),
                    &streamed_ids,
                )
            }
            AttachmentContent::Binary(_) => {
                UnicodeAttachment::read(top.clone(), attachment_node, None)
            }
        }
        .map_err(|error| {
            invalid(&format!(
                "completed store attachment {index} cannot be read: {error}"
            ))
        })?;
        let properties = attachment.properties();
        let row = attachment_table
            .find_row(crate::ltp::table_context::TableRowId::new(u32::from(
                attachment_node,
            )))
            .map_err(|_| invalid("completed store attachment row is missing"))?;
        let row_values = row.columns(attachment_table.context())?;
        let table_value =
            |property_id| -> Result<crate::ltp::prop_context::PropertyValue, WriterError> {
                let column = attachment_table
                    .context()
                    .columns()
                    .iter()
                    .position(|column| column.prop_id() == property_id)
                    .ok_or_else(|| invalid("completed store attachment column is missing"))?;
                let value = row_values[column]
                    .as_ref()
                    .ok_or_else(|| invalid("completed store attachment table value is missing"))?;
                Ok(attachment_table.read_column(
                    value,
                    attachment_table.context().columns()[column].prop_type(),
                )?)
            };
        let pc_size = properties.attachment_size()?;
        let pc_method = properties.attachment_method()?;
        let pc_rendering = properties.rendering_position()?;
        let expected_number =
            i32::try_from(index).map_err(|_| WriterError::ValueTooLarge("attachment number"))?;
        if !matches!(table_value(0x0E20)?, crate::ltp::prop_context::PropertyValue::Integer32(value) if value == pc_size)
            || !matches!(table_value(0x0E21)?, crate::ltp::prop_context::PropertyValue::Integer32(value) if value == expected_number)
            || !matches!(properties.get(0x0E21), Some(crate::ltp::prop_context::PropertyValue::Integer32(value)) if *value == expected_number)
            || !matches!(table_value(0x3704)?, crate::ltp::prop_context::PropertyValue::Unicode(value) if value.to_string() == expected.filename)
            || !matches!(table_value(0x3705)?, crate::ltp::prop_context::PropertyValue::Integer32(value) if value == pc_method)
            || !matches!(table_value(0x370B)?, crate::ltp::prop_context::PropertyValue::Integer32(value) if value == pc_rendering)
        {
            return Err(invalid("completed store attachment table value mismatch"));
        }
        let unicode_matches = |id, value: &Option<String>| match value {
            Some(expected) => matches!(
                properties.get(id),
                Some(crate::ltp::prop_context::PropertyValue::Unicode(actual))
                    if actual.to_string() == *expected
            ),
            None => properties.get(id).is_none(),
        };
        if !matches!(
            properties.get(0x3704),
            Some(crate::ltp::prop_context::PropertyValue::Unicode(value))
                if value.to_string() == expected.filename
        ) || !matches!(
            properties.get(0x3707),
            Some(crate::ltp::prop_context::PropertyValue::Unicode(value))
                if value.to_string() == expected.filename
        ) || !unicode_matches(0x370E, &expected.mime_type)
            || !unicode_matches(0x3712, &expected.content_id)
            || !unicode_matches(0x3713, &expected.content_location)
            || properties.rendering_position()? != expected.rendering_position.unwrap_or(-1)
            || !matches!(
                properties.get(0x3714),
                Some(crate::ltp::prop_context::PropertyValue::Integer32(value))
                    if *value == expected.flags
            )
        {
            return Err(invalid("completed store attachment metadata mismatch"));
        }
        for property in &expected.raw_properties {
            if !properties
                .get(property.id)
                .is_some_and(|actual| raw_value_matches(&property.value, actual))
            {
                return Err(invalid("completed store attachment raw-property mismatch"));
            }
        }
        match (&expected.content, attachment.data()) {
            (AttachmentContent::Binary(expected_data), Some(AttachmentData::Binary(actual))) => {
                let expected_size = attachment_property_size(&attachment_properties(
                    expected,
                    expected_number,
                    1,
                    0,
                    PropertyValue::Binary(expected_data.clone()),
                ))?;
                if actual.buffer() != expected_data || pc_method != 1 || pc_size != expected_size {
                    return Err(invalid("completed store binary attachment size mismatch"));
                }
            }
            (AttachmentContent::Spooled(blob), None) => {
                let expected_size = attachment_property_size_with_stream(
                    &attachment_properties(
                        expected,
                        expected_number,
                        1,
                        0,
                        PropertyValue::External(
                            PropertyType::Binary,
                            node(NodeIdType::ListsTablesProperties, 1)?,
                        ),
                    ),
                    Some(blob.byte_len),
                )?;
                if pc_method != 1
                    || pc_size != expected_size
                    || attachment.streamed_data_identity() != Some((blob.byte_len, blob.sha256))
                {
                    return Err(invalid("completed store spooled attachment size mismatch"));
                }
            }
            (
                AttachmentContent::Embedded(expected_message),
                Some(AttachmentData::Message(actual)),
            ) => {
                let embedded_node = node(
                    NodeIdType::NormalMessage,
                    0x3_0000_u32
                        .checked_add(index_u32)
                        .ok_or(WriterError::ValueTooLarge("embedded message node"))?,
                )?;
                let embedded_size = actual.properties().message_size()?;
                let expected_size = attachment_property_size(&attachment_properties(
                    expected,
                    expected_number,
                    5,
                    0,
                    PropertyValue::Object(
                        embedded_node,
                        u32::try_from(embedded_size)
                            .map_err(|_| WriterError::ValueTooLarge("embedded message"))?,
                    ),
                ))?;
                if pc_method != 5 || pc_size != expected_size {
                    return Err(invalid("completed store embedded attachment size mismatch"));
                }
                validate_embedded_message(
                    actual.clone(),
                    expected_message,
                    named_identities,
                    embedded_message_record_key(
                        message_record_key(spec.record_key, top_node),
                        index_u32,
                    ),
                )?;
            }
            _ => return Err(invalid("completed store attachment content mismatch")),
        }
    }
    Ok(())
}

fn validate_named_map(
    actual: &dyn crate::messaging::named_prop::NamedPropertyMap,
    expected: &[NamedIdentity],
) -> Result<(), WriterError> {
    use crate::{
        ltp::prop_context::PropertyValue as ReadValue,
        messaging::named_prop::{NamedPropertyGuid, NamedPropertyId},
    };

    let invalid = |message: &str| WriterError::InvalidStructure(message.to_owned());
    let properties = actual.properties();
    let expected_properties = named_property_map(expected)?;
    if properties.iter().count() != expected_properties.len() {
        return Err(invalid("completed store NAMEID property count mismatch"));
    }
    for (id, expected) in expected_properties {
        let matches = match (expected, properties.get(id)) {
            (PropertyValue::Integer32(left), Some(ReadValue::Integer32(right))) => left == *right,
            (PropertyValue::Binary(left), Some(ReadValue::Binary(right))) => left == right.buffer(),
            (PropertyValue::Binary(left), Some(ReadValue::Null)) => left.is_empty(),
            _ => false,
        };
        if !matches {
            return Err(invalid(&format!(
                "completed store NAMEID stream or bucket 0x{id:04X} mismatch"
            )));
        }
    }

    if expected.is_empty() {
        return Ok(());
    }

    let entries = properties.stream_entry()?;
    if entries.len() != expected.len() {
        return Err(invalid("completed store NAMEID entry count mismatch"));
    }
    for (index, (entry, (expected_set, expected_name))) in entries.iter().zip(expected).enumerate()
    {
        let expected_id = 0x8000_u16
            .checked_add(u16::try_from(index).map_err(|_| invalid("NAMEID index overflow"))?)
            .ok_or_else(|| invalid("NAMEID index overflow"))?;
        if entry.prop_id() != expected_id {
            return Err(invalid("completed store NAMEID property index mismatch"));
        }
        match (entry.id(), expected_name) {
            (NamedPropertyId::Number(actual), NamedPropertyName::Numeric(expected))
                if actual == *expected => {}
            (NamedPropertyId::StringOffset(offset), NamedPropertyName::String(expected))
                if properties.lookup_string(offset)?.to_string() == *expected => {}
            _ => return Err(invalid("completed store NAMEID name mismatch")),
        }
        match (entry.guid(), expected_set) {
            (NamedPropertyGuid::Mapi, NamedPropertySet::Mapi)
            | (NamedPropertyGuid::PublicStrings, NamedPropertySet::PublicStrings) => {}
            (NamedPropertyGuid::GuidIndex(_), NamedPropertySet::Guid(expected))
                if properties.lookup_guid(entry.guid())?.to_le_bytes() == *expected => {}
            _ => return Err(invalid("completed store NAMEID GUID mismatch")),
        }
    }
    Ok(())
}

fn validate_embedded_message(
    actual: std::rc::Rc<dyn crate::messaging::message::Message>,
    expected: &MessageSpec,
    named_identities: &[NamedIdentity],
    record_key: [u8; 16],
) -> Result<(), WriterError> {
    use crate::ltp::prop_context::PropertyValue as ReadValue;

    let invalid = |message: &str| WriterError::InvalidStructure(message.to_owned());
    let properties = actual.properties();
    let unicode_matches = |id, expected: &str| matches!(properties.get(id), Some(ReadValue::Unicode(value)) if value.to_string() == expected);
    let sender_matches = if expected.sender_name.is_empty() && expected.sender_email.is_empty() {
        [0x0042, 0x0064, 0x0065, 0x0C1A, 0x0C1E, 0x0C1F]
            .into_iter()
            .all(|id| properties.get(id).is_none())
    } else {
        unicode_matches(0x0042, &expected.sender_name)
            && unicode_matches(0x0065, &expected.sender_email)
            && unicode_matches(0x0C1A, &expected.sender_name)
            && unicode_matches(0x0C1F, &expected.sender_email)
            && unicode_matches(0x0064, "SMTP")
            && unicode_matches(0x0C1E, "SMTP")
    };
    if properties.message_class()? != expected.message_class
        || !unicode_matches(0x0037, &expected.subject)
        || !sender_matches
        || !matches!(properties.get(0x0039), Some(ReadValue::Time(value)) if *value == expected.sent_filetime)
        || !matches!(properties.get(0x0E06), Some(ReadValue::Time(value)) if *value == expected.received_filetime)
        || !matches!(properties.get(0x3007), Some(ReadValue::Time(value)) if *value == expected.creation_filetime)
        || !matches!(properties.get(0x3008), Some(ReadValue::Time(value)) if *value == expected.modification_filetime)
        || !matches!(properties.get(0x300B), Some(ReadValue::Binary(value)) if value.buffer() == record_key)
        || !matches!(properties.get(0x0E07), Some(ReadValue::Integer32(value)) if *value == output_message_flags(expected, false))
        || !matches!(properties.get(0x0E1B), Some(ReadValue::Boolean(value)) if *value != expected.attachments.is_empty())
        || !matches!(properties.get(0x3FDE), Some(ReadValue::Integer32(value)) if *value == expected.internet_codepage)
    {
        return Err(invalid("completed store embedded metadata mismatch"));
    }
    match (&expected.body_text, properties.get(0x1000)) {
        (Some(expected), Some(ReadValue::Unicode(actual))) if actual.to_string() == *expected => {}
        (None, None) => {}
        _ => return Err(invalid("completed store embedded text mismatch")),
    }
    match (&expected.body_html, properties.get(0x1013)) {
        (Some(expected), Some(ReadValue::Binary(actual))) if actual.buffer() == expected => {}
        (None, None) => {}
        _ => return Err(invalid("completed store embedded HTML mismatch")),
    }
    match (&expected.body_rtf, properties.get(0x1009)) {
        (Some(expected), Some(ReadValue::Binary(actual)))
            if actual.buffer() == rtf_container(expected)? => {}
        (None, None) => {}
        _ => return Err(invalid("completed store embedded RTF mismatch")),
    }
    let has_streamed_rtf = expected
        .spooled_properties
        .iter()
        .any(|property| property.id == 0x1009);
    match (
        expected.body_rtf.is_some() || has_streamed_rtf,
        properties.get(0x0E1F),
    ) {
        (true, Some(ReadValue::Boolean(actual))) if *actual == expected.rtf_in_sync => {}
        (false, None) => {}
        _ => return Err(invalid("completed store embedded RTF sync mismatch")),
    }
    match (expected.native_body, properties.get(0x1016)) {
        (Some(expected), Some(ReadValue::Integer32(actual))) if *actual == expected as i32 => {}
        (None, None) => {}
        _ => return Err(invalid("completed store embedded native body mismatch")),
    }
    match (&expected.internet_headers, properties.get(0x007D)) {
        (Some(expected), Some(ReadValue::Unicode(actual))) if actual.to_string() == *expected => {}
        (None, None) => {}
        _ => return Err(invalid("completed store embedded headers mismatch")),
    }
    let expected_display = display_recipient_properties(&expected.recipients);
    for id in [0x0E02, 0x0E03, 0x0E04] {
        let expected = expected_display
            .iter()
            .find_map(|(property_id, value)| (*property_id == id).then_some(value));
        match (expected, properties.get(id)) {
            (Some(PropertyValue::Unicode(expected)), Some(ReadValue::Unicode(actual)))
                if actual.to_string() == *expected => {}
            (None, None) => {}
            _ => {
                return Err(invalid(
                    "completed store embedded display-recipient mismatch",
                ));
            }
        }
    }
    for property in &expected.named_properties {
        let index = named_identities
            .binary_search(&(property.set, property.name.clone()))
            .map_err(|_| invalid("embedded named property is not mapped"))?;
        let id = 0x8000_u16
            .checked_add(u16::try_from(index).map_err(|_| invalid("named property overflow"))?)
            .ok_or_else(|| invalid("named property overflow"))?;
        if !properties
            .get(id)
            .is_some_and(|actual| raw_value_matches(&property.value, actual))
        {
            return Err(invalid("completed store embedded named property mismatch"));
        }
    }
    for property in &expected.raw_properties {
        if !properties
            .get(property.id)
            .is_some_and(|actual| raw_value_matches(&property.value, actual))
        {
            return Err(invalid("completed store embedded raw property mismatch"));
        }
    }
    for property in &expected.spooled_properties {
        if actual.streamed_property_identity(property.id)
            != Some((
                property.property_type,
                property.blob.byte_len,
                property.blob.sha256,
            ))
        {
            return Err(invalid(
                "completed store embedded streamed property mismatch",
            ));
        }
    }
    let recipients = actual
        .recipient_table()
        .ok_or_else(|| invalid("completed store embedded recipient table is missing"))?;
    if recipients
        .find_row(crate::ltp::table_context::TableRowId::new(0))
        .is_ok()
    {
        return Err(invalid(
            "completed store embedded recipient table contains row ID zero",
        ));
    }
    if recipients.rows_matrix().count() != expected.recipients.len() {
        return Err(invalid("completed store embedded recipient count mismatch"));
    }
    let attachments = actual
        .attachment_table()
        .ok_or_else(|| invalid("completed store embedded attachment table is missing"))?;
    if attachments.rows_matrix().count() != expected.attachments.len() {
        return Err(invalid(
            "completed store embedded attachment count mismatch",
        ));
    }
    let columns = recipients.context().columns();
    let column = |id| {
        columns
            .iter()
            .position(|column| column.prop_id() == id)
            .ok_or_else(|| invalid("embedded recipient column is missing"))
    };
    let role = column(0x0C15)?;
    let name = column(0x3001)?;
    let address_type = column(0x3002)?;
    let email = column(0x3003)?;
    let smtp = column(0x39FF)?;
    for (index, (row, expected)) in recipients
        .rows_matrix()
        .zip(&expected.recipients)
        .enumerate()
    {
        let expected_row = u32::try_from(index)
            .ok()
            .and_then(|row| row.checked_add(1))
            .ok_or_else(|| invalid("embedded recipient row ID overflow"))?;
        let expected_row = crate::ltp::table_context::TableRowId::new(expected_row);
        let indexed_row = recipients
            .find_row(expected_row)
            .map_err(|_| invalid("embedded recipient row is not indexed"))?;
        if row.id() != expected_row || indexed_row.id() != expected_row {
            return Err(invalid("embedded recipient row ID mismatch"));
        }
        let values = row.columns(recipients.context())?;
        let read = |index: usize| -> Result<ReadValue, WriterError> {
            Ok(recipients.read_column(
                values[index]
                    .as_ref()
                    .ok_or_else(|| invalid("embedded recipient value is missing"))?,
                columns[index].prop_type(),
            )?)
        };
        if !matches!(read(role)?, ReadValue::Integer32(value) if value == expected.kind as i32)
            || !matches!(read(name)?, ReadValue::Unicode(value) if value.to_string() == expected.display_name)
            || !matches!(read(address_type)?, ReadValue::Unicode(value) if value.to_string() == "SMTP")
            || !matches!(read(email)?, ReadValue::Unicode(value) if value.to_string() == expected.email_address)
            || !matches!(read(smtp)?, ReadValue::Unicode(value) if value.to_string() == expected.email_address)
        {
            return Err(invalid("completed store embedded recipient mismatch"));
        }
    }
    for (index, expected_attachment) in expected.attachments.iter().enumerate() {
        use crate::messaging::attachment::AttachmentData;

        let index_u32 =
            u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("attachment count"))?;
        let attachment_node = node(
            NodeIdType::Attachment,
            0x2_0000_u32
                .checked_add(index_u32)
                .ok_or(WriterError::ValueTooLarge("attachment node"))?,
        )?;
        let validation_ids = match &expected_attachment.content {
            AttachmentContent::Embedded(message) => {
                Some(validation_property_ids(message, named_identities)?)
            }
            AttachmentContent::Binary(_) | AttachmentContent::Spooled(_) => None,
        };
        let streamed_ids = match &expected_attachment.content {
            AttachmentContent::Embedded(message) => message
                .spooled_properties
                .iter()
                .map(|property| property.id)
                .collect::<Vec<_>>(),
            AttachmentContent::Binary(_) | AttachmentContent::Spooled(_) => Vec::new(),
        };
        let attachment = actual
            .clone()
            .read_attachment(
                attachment_node,
                validation_ids.as_deref(),
                &streamed_ids,
                !matches!(&expected_attachment.content, AttachmentContent::Spooled(_)),
            )
            .map_err(|error| {
                invalid(&format!(
                    "completed store nested attachment {index} cannot be read: {error}"
                ))
            })?;
        let properties = attachment.properties();
        let row = attachments
            .find_row(crate::ltp::table_context::TableRowId::new(u32::from(
                attachment_node,
            )))
            .map_err(|_| invalid("completed store nested attachment row is missing"))?;
        let row_values = row.columns(attachments.context())?;
        let table_value = |property_id| -> Result<ReadValue, WriterError> {
            let column = attachments
                .context()
                .columns()
                .iter()
                .position(|column| column.prop_id() == property_id)
                .ok_or_else(|| invalid("completed store nested attachment column is missing"))?;
            let value = row_values[column]
                .as_ref()
                .ok_or_else(|| invalid("completed store nested attachment value is missing"))?;
            Ok(attachments
                .read_column(value, attachments.context().columns()[column].prop_type())?)
        };
        let expected_number =
            i32::try_from(index).map_err(|_| WriterError::ValueTooLarge("attachment number"))?;
        let pc_size = properties.attachment_size()?;
        let pc_method = properties.attachment_method()?;
        let pc_rendering = properties.rendering_position()?;
        if !matches!(table_value(0x0E20)?, ReadValue::Integer32(value) if value == pc_size)
            || !matches!(table_value(0x0E21)?, ReadValue::Integer32(value) if value == expected_number)
            || !matches!(properties.get(0x0E21), Some(ReadValue::Integer32(value)) if *value == expected_number)
            || !matches!(table_value(0x3704)?, ReadValue::Unicode(value) if value.to_string() == expected_attachment.filename)
            || !matches!(table_value(0x3705)?, ReadValue::Integer32(value) if value == pc_method)
            || !matches!(table_value(0x370B)?, ReadValue::Integer32(value) if value == pc_rendering)
        {
            return Err(invalid(
                "completed store nested attachment table value mismatch",
            ));
        }
        let unicode_matches = |id, value: &Option<String>| match value {
            Some(expected) => matches!(
                properties.get(id),
                Some(ReadValue::Unicode(actual)) if actual.to_string() == *expected
            ),
            None => properties.get(id).is_none(),
        };
        if !matches!(
            properties.get(0x3704),
            Some(ReadValue::Unicode(value)) if value.to_string() == expected_attachment.filename
        ) || !matches!(
            properties.get(0x3707),
            Some(ReadValue::Unicode(value)) if value.to_string() == expected_attachment.filename
        ) || !unicode_matches(0x370E, &expected_attachment.mime_type)
            || !unicode_matches(0x3712, &expected_attachment.content_id)
            || !unicode_matches(0x3713, &expected_attachment.content_location)
            || pc_rendering != expected_attachment.rendering_position.unwrap_or(-1)
            || !matches!(
                properties.get(0x3714),
                Some(ReadValue::Integer32(value)) if *value == expected_attachment.flags
            )
        {
            return Err(invalid(
                "completed store nested attachment metadata mismatch",
            ));
        }
        for property in &expected_attachment.raw_properties {
            if !properties
                .get(property.id)
                .is_some_and(|actual| raw_value_matches(&property.value, actual))
            {
                return Err(invalid(
                    "completed store nested attachment raw-property mismatch",
                ));
            }
        }
        match (&expected_attachment.content, attachment.data()) {
            (AttachmentContent::Binary(expected), Some(AttachmentData::Binary(actual))) => {
                let expected_size = attachment_property_size(&attachment_properties(
                    expected_attachment,
                    expected_number,
                    1,
                    0,
                    PropertyValue::Binary(expected.clone()),
                ))?;
                if actual.buffer() != expected || pc_method != 1 || pc_size != expected_size {
                    return Err(invalid(
                        "completed store nested binary attachment size mismatch",
                    ));
                }
            }
            (AttachmentContent::Spooled(expected), None) => {
                let expected_size = attachment_property_size_with_stream(
                    &attachment_properties(
                        expected_attachment,
                        expected_number,
                        1,
                        0,
                        PropertyValue::External(
                            PropertyType::Binary,
                            node(NodeIdType::ListsTablesProperties, 1)?,
                        ),
                    ),
                    Some(expected.byte_len),
                )?;
                if pc_method != 1
                    || pc_size != expected_size
                    || attachment.streamed_data_identity()
                        != Some((expected.byte_len, expected.sha256))
                {
                    return Err(invalid(
                        "completed store nested spooled attachment size mismatch",
                    ));
                }
            }
            (
                AttachmentContent::Embedded(expected_child),
                Some(AttachmentData::Message(actual_child)),
            ) => {
                let embedded_node = node(
                    NodeIdType::NormalMessage,
                    0x3_0000_u32
                        .checked_add(index_u32)
                        .ok_or(WriterError::ValueTooLarge("embedded message node"))?,
                )?;
                let embedded_size = actual_child.properties().message_size()?;
                let expected_size = attachment_property_size(&attachment_properties(
                    expected_attachment,
                    expected_number,
                    5,
                    0,
                    PropertyValue::Object(
                        embedded_node,
                        u32::try_from(embedded_size)
                            .map_err(|_| WriterError::ValueTooLarge("embedded message"))?,
                    ),
                ))?;
                if pc_method != 5 || pc_size != expected_size {
                    return Err(invalid(
                        "completed store nested embedded attachment size mismatch",
                    ));
                }
                validate_embedded_message(
                    actual_child.clone(),
                    expected_child,
                    named_identities,
                    embedded_message_record_key(record_key, index_u32),
                )?;
            }
            _ => {
                return Err(invalid(
                    "completed store nested attachment content mismatch",
                ));
            }
        }
    }
    Ok(())
}

fn store_properties(
    spec: &StoreInput<'_>,
    ipm_folder: NodeId,
    deleted_folder: NodeId,
    search_root: NodeId,
) -> Result<Vec<(u16, PropertyValue)>, WriterError> {
    Ok(vec![
        (0x0E34, PropertyValue::Binary(spec.record_key.to_vec())),
        (0x0FF9, PropertyValue::Binary(spec.record_key.to_vec())),
        (0x3001, PropertyValue::Unicode(spec.store_name.to_owned())),
        (
            0x35E0,
            PropertyValue::Binary(entry_id(spec.record_key, ipm_folder)?),
        ),
        (
            0x35E3,
            PropertyValue::Binary(entry_id(spec.record_key, deleted_folder)?),
        ),
        (
            0x35E7,
            PropertyValue::Binary(entry_id(spec.record_key, search_root)?),
        ),
        (0x6633, PropertyValue::Boolean(true)),
        (0x67FF, PropertyValue::Integer32(0)),
    ])
}

fn folder_properties(
    name: &str,
    content_count: i32,
    has_children: bool,
) -> Vec<(u16, PropertyValue)> {
    folder_properties_with_unread(name, content_count, 0, has_children, "IPF.Note")
}

fn folder_properties_with_unread(
    name: &str,
    content_count: i32,
    unread_count: i32,
    has_children: bool,
    container_class: &str,
) -> Vec<(u16, PropertyValue)> {
    vec![
        (0x3001, PropertyValue::Unicode(name.to_owned())),
        (0x3601, PropertyValue::Integer32(1)),
        (0x3602, PropertyValue::Integer32(content_count)),
        (0x3603, PropertyValue::Integer32(unread_count)),
        (0x360A, PropertyValue::Boolean(has_children)),
        (0x3613, PropertyValue::Unicode(container_class.to_owned())),
    ]
}

type NamedIdentity = (NamedPropertySet, NamedPropertyName);

fn collect_named_identities(message: &MessageSpec) -> Vec<NamedIdentity> {
    fn collect<'a>(
        message: &'a MessageSpec,
        identities: &mut BTreeSet<(NamedPropertySet, &'a NamedPropertyName)>,
    ) {
        identities.extend(
            message
                .named_properties
                .iter()
                .map(|property| (property.set, &property.name)),
        );
        for attachment in &message.attachments {
            if let AttachmentContent::Embedded(embedded) = &attachment.content {
                collect(embedded, identities);
            }
        }
    }
    let mut identities = BTreeSet::new();
    collect(message, &mut identities);
    identities
        .into_iter()
        .map(|(set, name)| (set, name.clone()))
        .collect()
}

fn collect_named_identities_many_refs(messages: &[&MessageSpec]) -> Vec<NamedIdentity> {
    messages
        .iter()
        .flat_map(|message| collect_named_identities(message))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn named_property_map(named: &[NamedIdentity]) -> Result<Vec<(u16, PropertyValue)>, WriterError> {
    if named.is_empty() {
        // Outlook/libpff require the NAMEID entry and hash streams to be
        // present even when no message references a named property. Preserve
        // the interoperable reserved MAPI mapping emitted by v0.2.0.
        let mut entry = Vec::with_capacity(8);
        entry.extend_from_slice(&0x0000_8005_u32.to_le_bytes());
        entry.extend_from_slice(&0x0002_u16.to_le_bytes());
        entry.extend_from_slice(&0_u16.to_le_bytes());
        let guid = vec![
            0x28, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ];
        return Ok(vec![
            (0x0001, PropertyValue::Integer32(251)),
            (0x0002, PropertyValue::Binary(guid)),
            (0x0003, PropertyValue::Binary(entry.clone())),
            (0x0004, PropertyValue::Binary(Vec::new())),
            (0x1000, PropertyValue::Binary(entry)),
        ]);
    }

    let mut custom_guids = named
        .iter()
        .filter_map(|(set, _)| match set {
            NamedPropertySet::Guid(guid) => Some(*guid),
            NamedPropertySet::Mapi | NamedPropertySet::PublicStrings => None,
        })
        .collect::<Vec<_>>();
    custom_guids.sort();
    custom_guids.dedup();
    let mut entries = Vec::with_capacity(named.len().saturating_mul(8));
    let mut strings = Vec::new();
    let mut buckets = BTreeMap::<u16, Vec<u8>>::new();
    for (index, (set, name)) in named.iter().enumerate() {
        let guid = match set {
            NamedPropertySet::Mapi => 1_u16,
            NamedPropertySet::PublicStrings => 2_u16,
            NamedPropertySet::Guid(value) => {
                let position = custom_guids.binary_search(value).map_err(|_| {
                    WriterError::InvalidStructure("named GUID is not indexed".to_owned())
                })?;
                u16::try_from(position)
                    .ok()
                    .and_then(|position| position.checked_add(3))
                    .ok_or(WriterError::ValueTooLarge("named-property GUID count"))?
            }
        };
        let (identifier, guid_and_kind, hash_identifier) = match name {
            NamedPropertyName::Numeric(identifier) => (*identifier, guid << 1, *identifier),
            NamedPropertyName::String(name) => {
                let offset = u32::try_from(strings.len())
                    .map_err(|_| WriterError::ValueTooLarge("named-property string stream"))?;
                let encoded = unicode_bytes(name)?;
                strings.extend_from_slice(
                    &u32::try_from(encoded.len())
                        .map_err(|_| WriterError::ValueTooLarge("named-property name"))?
                        .to_le_bytes(),
                );
                strings.extend_from_slice(&encoded);
                while strings.len() % 4 != 0 {
                    strings.push(0);
                }
                (
                    offset,
                    (guid << 1) | 1,
                    crate::crc::compute_crc(0, &encoded),
                )
            }
        };
        let property_index =
            u16::try_from(index).map_err(|_| WriterError::ValueTooLarge("named-property count"))?;
        let mut entry = Vec::with_capacity(8);
        entry.extend_from_slice(&identifier.to_le_bytes());
        entry.extend_from_slice(&guid_and_kind.to_le_bytes());
        entry.extend_from_slice(&property_index.to_le_bytes());
        entries.extend_from_slice(&entry);
        let bucket = u16::try_from((hash_identifier ^ u32::from(guid_and_kind)) % 251)
            .map_err(|_| WriterError::ValueTooLarge("named-property bucket"))?;
        buckets.entry(bucket).or_default().extend_from_slice(&entry);
    }
    let guid = if custom_guids.is_empty() {
        // libpff treats a zero-length GUID stream as a missing required stream.
        vec![
            0x28, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ]
    } else {
        custom_guids.into_iter().flatten().collect()
    };
    let mut properties = vec![
        (0x0001, PropertyValue::Integer32(251)),
        (0x0002, PropertyValue::Binary(guid)),
        (0x0003, PropertyValue::Binary(entries)),
        (0x0004, PropertyValue::Binary(strings)),
    ];
    properties.extend(
        buckets
            .into_iter()
            .map(|(bucket, entries)| (0x1000 + bucket, PropertyValue::Binary(entries))),
    );
    Ok(properties)
}

fn message_properties(
    message: &MessageSpec,
    associated: bool,
    named_identities: &[NamedIdentity],
    record_key: [u8; 16],
    message_size: i32,
) -> Result<Vec<(u16, PropertyValue)>, WriterError> {
    let mut properties = vec![
        (
            0x001A,
            PropertyValue::Unicode(message.message_class.clone()),
        ),
        (0x0037, PropertyValue::Unicode(message.subject.clone())),
        (0x0E06, PropertyValue::Time(message.received_filetime)),
        (
            0x0E07,
            PropertyValue::Integer32(output_message_flags(message, associated)),
        ),
        (0x0E08, PropertyValue::Integer32(message_size)),
        (0x0E17, PropertyValue::Integer32(0)),
        (
            0x0E1B,
            PropertyValue::Boolean(!message.attachments.is_empty()),
        ),
        (0x3007, PropertyValue::Time(message.creation_filetime)),
        (0x3008, PropertyValue::Time(message.modification_filetime)),
        (0x300B, PropertyValue::Binary(record_key.to_vec())),
        (0x3FDE, PropertyValue::Integer32(message.internet_codepage)),
    ];
    if !message.sender_name.is_empty() && !message.sender_email.is_empty() {
        properties.extend([
            (0x0042, PropertyValue::Unicode(message.sender_name.clone())),
            (0x0064, PropertyValue::Unicode("SMTP".to_owned())),
            (0x0065, PropertyValue::Unicode(message.sender_email.clone())),
            (0x0C1A, PropertyValue::Unicode(message.sender_name.clone())),
            (0x0C1E, PropertyValue::Unicode("SMTP".to_owned())),
            (0x0C1F, PropertyValue::Unicode(message.sender_email.clone())),
        ]);
    }
    if let Some(body) = &message.body_text {
        properties.push((0x1000, PropertyValue::Unicode(body.clone())));
    }
    if let Some(html) = &message.body_html {
        properties.push((0x1013, PropertyValue::Binary(html.clone())));
    }
    if let Some(rtf) = &message.body_rtf {
        properties.push((0x1009, PropertyValue::Binary(rtf_container(rtf)?)));
        properties.push((0x0E1F, PropertyValue::Boolean(message.rtf_in_sync)));
    } else if message
        .spooled_properties
        .iter()
        .any(|property| property.id == 0x1009)
    {
        properties.push((0x0E1F, PropertyValue::Boolean(message.rtf_in_sync)));
    }
    if let Some(native_body) = message.native_body {
        properties.push((0x1016, PropertyValue::Integer32(native_body as i32)));
    }
    if let Some(headers) = &message.internet_headers {
        properties.push((0x007D, PropertyValue::Unicode(headers.clone())));
    }
    properties.push((0x0039, PropertyValue::Time(message.sent_filetime)));
    properties.extend(display_recipient_properties(&message.recipients));
    for property in &message.named_properties {
        let index = named_identities
            .binary_search(&(property.set, property.name.clone()))
            .map_err(|_| {
                WriterError::InvalidStructure("named property is not mapped".to_owned())
            })?;
        let id = 0x8000_u16
            .checked_add(
                u16::try_from(index)
                    .map_err(|_| WriterError::ValueTooLarge("named-property count"))?,
            )
            .ok_or(WriterError::ValueTooLarge("named-property identifier"))?;
        properties.push((id, raw_property_value(&property.value)));
    }
    for raw in &message.raw_properties {
        properties.push((raw.id, raw_property_value(&raw.value)));
    }
    Ok(properties)
}

fn output_message_flags(message: &MessageSpec, associated: bool) -> i32 {
    const HAS_ATTACHMENTS: i32 = 0x10;
    let mut flags = message.message_flags & !(HAS_ATTACHMENTS | MSGFLAG_ASSOCIATED);
    if !message.attachments.is_empty() {
        flags |= HAS_ATTACHMENTS;
    }
    if associated {
        flags |= MSGFLAG_ASSOCIATED;
    }
    flags
}

fn display_recipient_properties(recipients: &[RecipientSpec]) -> Vec<(u16, PropertyValue)> {
    [
        (RecipientKind::To, 0x0E04),
        (RecipientKind::Cc, 0x0E03),
        (RecipientKind::Bcc, 0x0E02),
    ]
    .into_iter()
    .filter_map(|(kind, id)| {
        let display = recipients
            .iter()
            .filter(|recipient| recipient.kind == kind)
            .map(|recipient| recipient.display_name.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        (!display.is_empty()).then_some((id, PropertyValue::Unicode(display)))
    })
    .collect()
}

fn raw_property_value(value: &RawPropertyValue) -> PropertyValue {
    match value {
        RawPropertyValue::Integer16(value) => PropertyValue::Integer16(*value),
        RawPropertyValue::Integer32(value) => PropertyValue::Integer32(*value),
        RawPropertyValue::Integer64(value) => PropertyValue::Integer64(*value),
        RawPropertyValue::Floating32(value) => PropertyValue::Floating32(*value),
        RawPropertyValue::Floating64(value) => PropertyValue::Floating64(*value),
        RawPropertyValue::Currency(value) => PropertyValue::Currency(*value),
        RawPropertyValue::FloatingTime(value) => PropertyValue::FloatingTime(*value),
        RawPropertyValue::ErrorCode(value) => PropertyValue::ErrorCode(*value),
        RawPropertyValue::Boolean(value) => PropertyValue::Boolean(*value),
        RawPropertyValue::Time(value) => PropertyValue::Time(*value),
        RawPropertyValue::Guid(value) => PropertyValue::Guid(*value),
        RawPropertyValue::Unicode(value) => PropertyValue::Unicode(value.clone()),
        RawPropertyValue::Binary(value) => PropertyValue::Binary(value.clone()),
        RawPropertyValue::MultipleInteger16(value) => {
            PropertyValue::MultipleInteger16(value.clone())
        }
        RawPropertyValue::MultipleInteger32(value) => {
            PropertyValue::MultipleInteger32(value.clone())
        }
        RawPropertyValue::MultipleInteger64(value) => {
            PropertyValue::MultipleInteger64(value.clone())
        }
        RawPropertyValue::MultipleGuid(value) => PropertyValue::MultipleGuid(value.clone()),
    }
}

fn rtf_container_len(raw_size: usize) -> Result<usize, WriterError> {
    let runs = raw_size
        .checked_add(1)
        .ok_or(WriterError::ValueTooLarge("RTF body"))?
        .div_ceil(8);
    raw_size
        .checked_add(runs)
        .and_then(|size| size.checked_add(18))
        .ok_or(WriterError::ValueTooLarge("RTF body"))
}

fn rtf_container(rtf: &[u8]) -> Result<Vec<u8>, WriterError> {
    const INITIAL_DICTIONARY_SIZE: usize = 207;
    const DICTIONARY_SIZE: usize = 4096;

    let capacity = rtf_container_len(rtf.len())?;
    let mut compressed = Vec::with_capacity(capacity.saturating_sub(16));
    let complete_runs = rtf.len() / 8;
    for chunk in rtf[..complete_runs * 8].chunks_exact(8) {
        compressed.push(0);
        compressed.extend_from_slice(chunk);
    }
    let remainder = &rtf[complete_runs * 8..];
    compressed.push(1_u8 << remainder.len());
    compressed.extend_from_slice(remainder);
    let end_offset = (INITIAL_DICTIONARY_SIZE + rtf.len()) % DICTIONARY_SIZE;
    let end_reference = u16::try_from(end_offset)
        .map_err(|_| WriterError::ValueTooLarge("RTF dictionary offset"))?
        << 4;
    compressed.extend_from_slice(&end_reference.to_be_bytes());

    let compressed_size = u32::try_from(compressed.len().saturating_add(12))
        .map_err(|_| WriterError::ValueTooLarge("compressed RTF"))?;
    let raw_size = u32::try_from(rtf.len()).map_err(|_| WriterError::ValueTooLarge("raw RTF"))?;
    let mut bytes = Vec::with_capacity(compressed.len().saturating_add(16));
    bytes.extend_from_slice(&compressed_size.to_le_bytes());
    bytes.extend_from_slice(&raw_size.to_le_bytes());
    bytes.extend_from_slice(&0x7546_5A4C_u32.to_le_bytes());
    bytes.extend_from_slice(&crate::crc::compute_crc(0, &compressed).to_le_bytes());
    bytes.extend_from_slice(&compressed);
    debug_assert_eq!(bytes.len(), capacity);
    Ok(bytes)
}

fn property_context(properties: &[(u16, PropertyValue)]) -> Result<Vec<u8>, WriterError> {
    let mut sorted = properties.to_vec();
    sorted.sort_by_key(|(id, _)| *id);

    let mut allocations = Vec::<Vec<u8>>::new();
    allocations.push(Vec::new());
    allocations.push(Vec::new());

    let mut records = Vec::with_capacity(sorted.len().saturating_mul(8));
    for (property_id, value) in sorted {
        records.write_u16::<LittleEndian>(property_id)?;
        records.write_u16::<LittleEndian>(u16::from(value.property_type()))?;
        if let Some(inline) = value.inline_value() {
            records.write_u32::<LittleEndian>(inline)?;
        } else {
            let bytes = value.variable_bytes()?.ok_or_else(|| {
                WriterError::InvalidStructure("property has no serialized value".to_owned())
            })?;
            if bytes.is_empty() {
                records.write_u32::<LittleEndian>(0)?;
                continue;
            }
            allocations.push(bytes);
            let index = u16::try_from(allocations.len())
                .map_err(|_| WriterError::ValueTooLarge("property allocation count"))?;
            let heap_id = HeapId::new(index, 0)
                .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
            records.write_u32::<LittleEndian>(u32::from(heap_id))?;
        }
    }
    allocations[1] = records;

    let root =
        HeapId::new(2, 0).map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
    let mut tree_header = Vec::new();
    HeapTreeHeader::new(2, 6, 0, root)
        .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
        .write(&mut tree_header)?;
    allocations[0] = tree_header;

    heap_page(HeapNodeType::Properties, &allocations)
}

fn hierarchy_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Binary, 0x0E30),
        (PropertyType::Integer64, 0x0E33),
        (PropertyType::Binary, 0x0E34),
        (PropertyType::Integer32, 0x0E38),
        (PropertyType::Unicode, 0x3001),
        (PropertyType::Integer32, 0x3602),
        (PropertyType::Integer32, 0x3603),
        (PropertyType::Boolean, 0x360A),
        (PropertyType::Unicode, 0x3613),
        (PropertyType::Integer32, 0x6635),
        (PropertyType::Integer32, 0x6636),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

fn contents_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Integer32, 0x0017),
        (PropertyType::Unicode, 0x001A),
        (PropertyType::Integer32, 0x0036),
        (PropertyType::Unicode, 0x0037),
        (PropertyType::Time, 0x0039),
        (PropertyType::Unicode, 0x0042),
        (PropertyType::Boolean, 0x0057),
        (PropertyType::Boolean, 0x0058),
        (PropertyType::Unicode, 0x0070),
        (PropertyType::Binary, 0x0071),
        (PropertyType::Unicode, 0x0E03),
        (PropertyType::Unicode, 0x0E04),
        (PropertyType::Time, 0x0E06),
        (PropertyType::Integer32, 0x0E07),
        (PropertyType::Integer32, 0x0E08),
        (PropertyType::Integer32, 0x0E17),
        (PropertyType::Binary, 0x0E30),
        (PropertyType::Integer64, 0x0E33),
        (PropertyType::Binary, 0x0E34),
        (PropertyType::Integer32, 0x0E38),
        (PropertyType::Binary, 0x0E3C),
        (PropertyType::Binary, 0x0E3D),
        (PropertyType::Integer32, 0x1097),
        (PropertyType::Time, 0x3008),
        (PropertyType::Binary, 0x3013),
        (PropertyType::Integer32, 0x65C6),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

fn associated_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Unicode, 0x001A),
        (PropertyType::Integer32, 0x0E07),
        (PropertyType::Integer32, 0x0E17),
        (PropertyType::Unicode, 0x3001),
        (PropertyType::Unicode, 0x6800),
        (PropertyType::Boolean, 0x6803),
        (PropertyType::MultipleInteger32, 0x6805),
        (PropertyType::Unicode, 0x682F),
        (PropertyType::Integer32, 0x7003),
        (PropertyType::Binary, 0x7004),
        (PropertyType::Binary, 0x7005),
        (PropertyType::Unicode, 0x7006),
        (PropertyType::Integer32, 0x7007),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

fn search_contents_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Integer32, 0x0017),
        (PropertyType::Unicode, 0x001A),
        (PropertyType::Integer32, 0x0036),
        (PropertyType::Unicode, 0x0037),
        (PropertyType::Unicode, 0x0042),
        (PropertyType::Boolean, 0x0057),
        (PropertyType::Boolean, 0x0058),
        (PropertyType::Unicode, 0x0E03),
        (PropertyType::Unicode, 0x0E04),
        (PropertyType::Unicode, 0x0E05),
        (PropertyType::Time, 0x0E06),
        (PropertyType::Integer32, 0x0E07),
        (PropertyType::Integer32, 0x0E08),
        (PropertyType::Integer32, 0x0E17),
        (PropertyType::Boolean, 0x0E2A),
        (PropertyType::Time, 0x3008),
        (PropertyType::Integer32, 0x67F1),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

fn receive_folder_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Unicode, 0x001A),
        (PropertyType::Integer32, 0x6605),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

fn outgoing_queue_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Time, 0x0039),
        (PropertyType::Integer32, 0x0E10),
        (PropertyType::Integer32, 0x0E14),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

fn contents_index_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Integer64, 0x0E33),
        (PropertyType::Binary, 0x0E37),
        (PropertyType::Integer32, 0x0E38),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

fn search_index_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Unicode, 0x001A),
        (PropertyType::Binary, 0x0E30),
        (PropertyType::Binary, 0x0E31),
        (PropertyType::Integer64, 0x0E33),
        (PropertyType::Binary, 0x0E34),
        (PropertyType::Integer32, 0x0E38),
        (PropertyType::Binary, 0x0E3E),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

fn attachment_index_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Integer64, 0x0E33),
        (PropertyType::Time, 0x3007),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

// HMP is an Outlook-maintained reserved heap format omitted from MS-PST. This is
// the deterministic empty-store map emitted by ScanPST for the fixed node graph.
fn hierarchy_map() -> Vec<u8> {
    vec![
        0x7C, 0x00, 0xEC, 0x9C, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00,
        0x00, 0xB5, 0x10, 0x04, 0x00, 0x60, 0x00, 0x00, 0x00, 0x28, 0x96, 0x04, 0xFA, 0x1C, 0x10,
        0x81, 0x4F, 0x92, 0xFA, 0x3A, 0xA3, 0xFE, 0xB4, 0xD5, 0xD8, 0x22, 0x80, 0x00, 0x00, 0x2C,
        0x21, 0x1D, 0x81, 0xC7, 0x5C, 0x17, 0x47, 0xA3, 0x21, 0x24, 0xF6, 0x7A, 0x06, 0x45, 0x38,
        0x42, 0x80, 0x00, 0x00, 0x3E, 0xEA, 0xDB, 0xBA, 0x44, 0x95, 0xC4, 0x43, 0x80, 0xCB, 0x47,
        0x20, 0xCC, 0x2E, 0xE7, 0xE2, 0x62, 0x80, 0x00, 0x00, 0x72, 0x3A, 0x41, 0x00, 0x72, 0xCB,
        0xA5, 0x47, 0xB4, 0x3D, 0x82, 0xE7, 0x7C, 0xAC, 0xBF, 0xFA, 0x24, 0x00, 0x20, 0x00, 0xCC,
        0xAE, 0x4D, 0x56, 0xC4, 0x2D, 0x24, 0x41, 0x8F, 0xDD, 0x2E, 0x99, 0xBE, 0x96, 0x8E, 0xF0,
        0x82, 0x80, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x0C, 0x00, 0x10, 0x00, 0x18, 0x00, 0x7C,
        0x00,
    ]
}

fn recipient_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Integer32, 0x0C15),
        (PropertyType::Boolean, 0x0E0F),
        (PropertyType::Binary, 0x0FF9),
        (PropertyType::Integer32, 0x0FFE),
        (PropertyType::Binary, 0x0FFF),
        (PropertyType::Unicode, 0x3001),
        (PropertyType::Unicode, 0x3002),
        (PropertyType::Unicode, 0x3003),
        (PropertyType::Binary, 0x300B),
        (PropertyType::Integer32, 0x3900),
        (PropertyType::Unicode, 0x39FF),
        (PropertyType::Boolean, 0x3A40),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

fn attachment_columns() -> Result<Vec<TableColumnDescriptor>, WriterError> {
    schema_columns(&[
        (PropertyType::Integer32, 0x0E20),
        (PropertyType::Integer32, 0x0E21),
        (PropertyType::Unicode, 0x3704),
        (PropertyType::Integer32, 0x3705),
        (PropertyType::Integer32, 0x370B),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
}

fn recipient_table_row(
    index: usize,
    recipient: &RecipientSpec,
) -> Result<TableRowSpec, WriterError> {
    let row = u32::try_from(index)
        .ok()
        .and_then(|row| row.checked_add(1))
        .ok_or(WriterError::ValueTooLarge("recipient row count"))?;
    Ok(TableRowSpec {
        id: NodeId::from(row),
        values: vec![
            (0x0C15, PropertyValue::Integer32(recipient.kind as i32)),
            (0x0E0F, PropertyValue::Boolean(false)),
            (0x0FFE, PropertyValue::Integer32(6)),
            (
                0x3001,
                PropertyValue::Unicode(recipient.display_name.clone()),
            ),
            (0x3002, PropertyValue::Unicode("SMTP".to_owned())),
            (
                0x3003,
                PropertyValue::Unicode(recipient.email_address.clone()),
            ),
            (
                0x39FF,
                PropertyValue::Unicode(recipient.email_address.clone()),
            ),
        ],
    })
}

fn attachment_table_row(
    node: NodeId,
    attachment: &AttachmentSpec,
    attachment_number: i32,
    method: i32,
    size: i32,
) -> TableRowSpec {
    TableRowSpec {
        id: node,
        values: vec![
            (0x0E20, PropertyValue::Integer32(size)),
            (0x0E21, PropertyValue::Integer32(attachment_number)),
            (0x3704, PropertyValue::Unicode(attachment.filename.clone())),
            (0x3705, PropertyValue::Integer32(method)),
            (
                0x370B,
                PropertyValue::Integer32(attachment.rendering_position.unwrap_or(-1)),
            ),
        ],
    }
}

fn attachment_properties(
    attachment: &AttachmentSpec,
    attachment_number: i32,
    method: i32,
    size: i32,
    data: PropertyValue,
) -> Vec<(u16, PropertyValue)> {
    let mut properties = vec![
        (0x0E20, PropertyValue::Integer32(size)),
        (0x0E21, PropertyValue::Integer32(attachment_number)),
        (0x3701, data),
        (0x3704, PropertyValue::Unicode(attachment.filename.clone())),
        (0x3705, PropertyValue::Integer32(method)),
        (0x3707, PropertyValue::Unicode(attachment.filename.clone())),
        (
            0x370B,
            PropertyValue::Integer32(attachment.rendering_position.unwrap_or(-1)),
        ),
    ];
    if let Some(mime_type) = &attachment.mime_type {
        properties.push((0x370E, PropertyValue::Unicode(mime_type.clone())));
    }
    if let Some(content_id) = &attachment.content_id {
        properties.push((0x3712, PropertyValue::Unicode(content_id.clone())));
    }
    if let Some(content_location) = &attachment.content_location {
        properties.push((0x3713, PropertyValue::Unicode(content_location.clone())));
    }
    properties.push((0x3714, PropertyValue::Integer32(attachment.flags)));
    for property in &attachment.raw_properties {
        properties.push((property.id, raw_property_value(&property.value)));
    }
    properties
}

fn attachment_property_size(properties: &[(u16, PropertyValue)]) -> Result<i32, WriterError> {
    attachment_property_size_with_stream(properties, None)
}

fn attachment_property_size_with_stream(
    properties: &[(u16, PropertyValue)],
    streamed_size: Option<u64>,
) -> Result<i32, WriterError> {
    let size = properties.iter().try_fold(0_usize, |total, (_, value)| {
        let value_size = match value {
            PropertyValue::Integer16(_) => 2,
            PropertyValue::Integer32(_) => 4,
            PropertyValue::Integer64(_)
            | PropertyValue::Floating64(_)
            | PropertyValue::Currency(_)
            | PropertyValue::FloatingTime(_)
            | PropertyValue::Time(_) => 8,
            PropertyValue::Floating32(_) | PropertyValue::ErrorCode(_) => 4,
            PropertyValue::Boolean(_) => 1,
            PropertyValue::Guid(_) => 16,
            PropertyValue::Unicode(value) => unicode_payload_len(value)?,
            PropertyValue::Binary(value) => value.len(),
            PropertyValue::Object(_, size) => usize::try_from(*size)
                .map_err(|_| WriterError::ValueTooLarge("attachment object"))?,
            PropertyValue::External(PropertyType::Binary, _) => {
                usize::try_from(streamed_size.ok_or_else(|| {
                    WriterError::InvalidStructure("streamed attachment size is missing".to_owned())
                })?)
                .map_err(|_| WriterError::ValueTooLarge("streamed attachment"))?
            }
            _ => {
                return Err(WriterError::InvalidStructure(
                    "unsupported attachment property type for size calculation".to_owned(),
                ));
            }
        };
        total
            .checked_add(value_size)
            .ok_or(WriterError::ValueTooLarge("attachment properties"))
    })?;
    i32::try_from(size).map_err(|_| WriterError::ValueTooLarge("attachment properties"))
}

fn set_attachment_size(
    properties: &mut [(u16, PropertyValue)],
    attachment_size: i32,
) -> Result<(), WriterError> {
    let value = properties
        .iter_mut()
        .find_map(|(id, value)| (*id == 0x0E20).then_some(value))
        .ok_or_else(|| {
            WriterError::InvalidStructure("attachment size property is missing".to_owned())
        })?;
    *value = PropertyValue::Integer32(attachment_size);
    Ok(())
}

fn schema_columns(
    specs: &[(PropertyType, u16)],
) -> Result<Vec<TableColumnDescriptor>, WriterError> {
    let mut specs = specs.to_vec();
    specs.sort_by_key(|(_, id)| *id);
    if specs.windows(2).any(|pair| pair[0].1 == pair[1].1) {
        return Err(WriterError::InvalidStructure(
            "table schema contains duplicate properties".to_owned(),
        ));
    }

    let mut offsets = vec![0_u16; specs.len()];
    let mut next = 8_u16;
    for (index, (kind, id)) in specs.iter().enumerate() {
        if *id == LTP_ROW_ID_PROP_ID {
            offsets[index] = 0;
        } else if *id == LTP_ROW_VERSION_PROP_ID {
            offsets[index] = 4;
        } else if !matches!(kind, PropertyType::Integer16 | PropertyType::Boolean) {
            offsets[index] = next;
            next = next
                .checked_add(u16::from(column_width(*kind)?))
                .ok_or(WriterError::ValueTooLarge("table row"))?;
        }
    }
    next = u16::try_from(align_up(u64::from(next), 4))
        .map_err(|_| WriterError::ValueTooLarge("table row"))?;
    for (index, (kind, id)) in specs.iter().enumerate() {
        if *id != LTP_ROW_ID_PROP_ID
            && *id != LTP_ROW_VERSION_PROP_ID
            && *kind == PropertyType::Integer16
        {
            offsets[index] = next;
            next = next
                .checked_add(2)
                .ok_or(WriterError::ValueTooLarge("table row"))?;
        }
    }
    next = u16::try_from(align_up(u64::from(next), 2))
        .map_err(|_| WriterError::ValueTooLarge("table row"))?;
    for (index, (kind, id)) in specs.iter().enumerate() {
        if *id != LTP_ROW_ID_PROP_ID
            && *id != LTP_ROW_VERSION_PROP_ID
            && *kind == PropertyType::Boolean
        {
            offsets[index] = next;
            next = next
                .checked_add(1)
                .ok_or(WriterError::ValueTooLarge("table row"))?;
        }
    }

    let mut next_bit = 2_u8;
    specs
        .into_iter()
        .zip(offsets)
        .map(|((kind, id), offset)| {
            let bit = if id == LTP_ROW_ID_PROP_ID {
                0
            } else if id == LTP_ROW_VERSION_PROP_ID {
                1
            } else {
                let bit = next_bit;
                next_bit = next_bit
                    .checked_add(1)
                    .ok_or(WriterError::ValueTooLarge("table column bitmap"))?;
                bit
            };
            Ok(column(kind, id, offset, column_width(kind)?, bit))
        })
        .collect()
}

fn column_width(kind: PropertyType) -> Result<u8, WriterError> {
    match kind {
        PropertyType::Boolean => Ok(1),
        PropertyType::Integer16 => Ok(2),
        PropertyType::Floating64
        | PropertyType::Currency
        | PropertyType::FloatingTime
        | PropertyType::Integer64
        | PropertyType::Time => Ok(8),
        PropertyType::Integer32
        | PropertyType::Floating32
        | PropertyType::ErrorCode
        | PropertyType::String8
        | PropertyType::Unicode
        | PropertyType::Guid
        | PropertyType::Binary
        | PropertyType::Object
        | PropertyType::MultipleInteger16
        | PropertyType::MultipleInteger32
        | PropertyType::MultipleFloating32
        | PropertyType::MultipleFloating64
        | PropertyType::MultipleCurrency
        | PropertyType::MultipleFloatingTime
        | PropertyType::MultipleInteger64
        | PropertyType::MultipleString8
        | PropertyType::MultipleUnicode
        | PropertyType::MultipleTime
        | PropertyType::MultipleGuid
        | PropertyType::MultipleBinary => Ok(4),
        _ => Err(WriterError::InvalidStructure(
            "unsupported table column type".to_owned(),
        )),
    }
}

fn column(
    prop_type: PropertyType,
    prop_id: u16,
    offset: u16,
    size: u8,
    bit: u8,
) -> TableColumnDescriptor {
    TableColumnDescriptor::new(prop_type, prop_id, offset, size, bit)
}

fn folder_table_row(id: NodeId, name: &str, count: i32, children: bool) -> TableRowSpec {
    folder_table_row_with_unread(id, name, count, 0, children, "IPF.Note")
}

fn folder_table_row_with_unread(
    id: NodeId,
    name: &str,
    count: i32,
    unread_count: i32,
    children: bool,
    container_class: &str,
) -> TableRowSpec {
    TableRowSpec {
        id,
        values: vec![
            (0x3001, PropertyValue::Unicode(name.to_owned())),
            (0x3602, PropertyValue::Integer32(count)),
            (0x3603, PropertyValue::Integer32(unread_count)),
            (0x360A, PropertyValue::Boolean(children)),
            (0x3613, PropertyValue::Unicode(container_class.to_owned())),
        ],
    }
}

fn message_table_row(
    id: NodeId,
    message: &MessageSpec,
    store_key: [u8; 16],
    record_key: [u8; 16],
    message_size: i32,
    columns: &[TableColumnDescriptor],
) -> Result<TableRowSpec, WriterError> {
    let mut values = vec![
        (
            0x001A,
            PropertyValue::Unicode(message.message_class.clone()),
        ),
        (0x0037, PropertyValue::Unicode(message.subject.clone())),
        (0x0039, PropertyValue::Time(message.sent_filetime)),
        (0x0E06, PropertyValue::Time(message.received_filetime)),
        (
            0x0E07,
            PropertyValue::Integer32(output_message_flags(message, false)),
        ),
        (0x0E08, PropertyValue::Integer32(message_size)),
        (0x0E17, PropertyValue::Integer32(0)),
        (0x0E30, PropertyValue::Binary(record_key.to_vec())),
        (0x0E33, PropertyValue::Integer64(0x90)),
        (
            0x0E34,
            PropertyValue::Binary(message_instance_entry_id(store_key)),
        ),
        (0x3008, PropertyValue::Time(message.modification_filetime)),
    ];
    if !message.sender_name.is_empty() {
        values.push((0x0042, PropertyValue::Unicode(message.sender_name.clone())));
    }
    values.extend(
        display_recipient_properties(&message.recipients)
            .into_iter()
            .filter(|(id, _)| matches!(*id, 0x0E03 | 0x0E04)),
    );
    for raw in &message.raw_properties {
        if values.iter().any(|(id, _)| *id == raw.id) {
            continue;
        }
        let Some(column) = columns.iter().find(|column| column.prop_id() == raw.id) else {
            continue;
        };
        let value = raw_property_value(&raw.value);
        if value.property_type() != column.prop_type() {
            return Err(WriterError::InvalidStructure(format!(
                "raw property 0x{:04X} is incompatible with the contents table",
                raw.id
            )));
        }
        values.push((raw.id, value));
    }
    Ok(TableRowSpec { id, values })
}

fn associated_message_table_row(
    id: NodeId,
    message: &MessageSpec,
    columns: &[TableColumnDescriptor],
) -> TableRowSpec {
    let mut values = vec![
        (
            0x001A,
            PropertyValue::Unicode(message.message_class.clone()),
        ),
        (
            0x0E07,
            PropertyValue::Integer32(output_message_flags(message, true)),
        ),
        (0x0E17, PropertyValue::Integer32(0)),
        (
            0x3001,
            PropertyValue::Unicode(associated_display_name(message).to_owned()),
        ),
    ];
    for raw in &message.raw_properties {
        if values.iter().any(|(property_id, _)| *property_id == raw.id) {
            continue;
        }
        let Some(column) = columns.iter().find(|column| column.prop_id() == raw.id) else {
            continue;
        };
        if column.prop_type() != raw_property_value(&raw.value).property_type() {
            continue;
        }
        values.push((raw.id, raw_property_value(&raw.value)));
    }
    TableRowSpec { id, values }
}

fn associated_display_name(message: &MessageSpec) -> &str {
    message
        .raw_properties
        .iter()
        .find_map(|property| match (&property.id, &property.value) {
            (0x3001, RawPropertyValue::Unicode(value)) => Some(value.as_str()),
            _ => None,
        })
        .unwrap_or(&message.subject)
}

fn folder_unread_count(messages: &[&MessageSpec]) -> Result<i32, WriterError> {
    const MSGFLAG_READ: i32 = 0x0000_0001;
    i32::try_from(
        messages
            .iter()
            .filter(|message| output_message_flags(message, false) & MSGFLAG_READ == 0)
            .count(),
    )
    .map_err(|_| WriterError::ValueTooLarge("folder unread count"))
}

fn set_message_size(
    properties: &mut [(u16, PropertyValue)],
    message_size: i32,
) -> Result<(), WriterError> {
    let value = properties
        .iter_mut()
        .find_map(|(id, value)| (*id == 0x0E08).then_some(value))
        .ok_or_else(|| {
            WriterError::InvalidStructure("message size property is missing".to_owned())
        })?;
    *value = PropertyValue::Integer32(message_size);
    Ok(())
}

fn message_record_key(store_key: [u8; 16], message: NodeId) -> [u8; 16] {
    let node = u32::from(message).to_le_bytes();
    let mut key = store_key;
    for (index, byte) in key.iter_mut().enumerate() {
        *byte ^= node[index % node.len()].wrapping_add(index as u8);
    }
    key
}

fn embedded_message_record_key(parent_key: [u8; 16], attachment_index: u32) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(b"PSTForge embedded record key");
    hasher.update(parent_key);
    hasher.update(attachment_index.to_le_bytes());
    let digest = hasher.finalize();
    let mut key = [0_u8; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

fn table_context(
    columns: &[TableColumnDescriptor],
    rows: &[TableRowSpec],
) -> Result<Vec<u8>, WriterError> {
    let mut rows = rows.iter().collect::<Vec<_>>();
    rows.sort_by_key(|row| u32::from(row.id));
    let end_4byte = columns
        .iter()
        .filter(|column| {
            !matches!(
                column.prop_type(),
                PropertyType::Integer16 | PropertyType::Boolean
            )
        })
        .map(|column| column.offset().saturating_add(u16::from(column.size())))
        .max()
        .unwrap_or(0);
    let end_4byte = u16::try_from(align_up(u64::from(end_4byte), 4))
        .map_err(|_| WriterError::ValueTooLarge("table row"))?;
    let end_2byte = columns
        .iter()
        .filter(|column| column.prop_type() == PropertyType::Integer16)
        .map(|column| column.offset().saturating_add(2))
        .max()
        .unwrap_or(end_4byte)
        .max(end_4byte);
    let end_1byte = columns
        .iter()
        .filter(|column| column.prop_type() == PropertyType::Boolean)
        .map(|column| column.offset().saturating_add(1))
        .max()
        .unwrap_or(end_2byte)
        .max(end_2byte);
    let end_bitmap = end_1byte
        .checked_add(
            u16::try_from(columns.len().div_ceil(8))
                .map_err(|_| WriterError::ValueTooLarge("table bitmap"))?,
        )
        .ok_or(WriterError::ValueTooLarge("table row"))?;
    let row_index =
        HeapId::new(2, 0).map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
    let row_tree_root = if rows.is_empty() {
        HeapId::from(0_u32)
    } else {
        HeapId::new(3, 0).map_err(|error| WriterError::InvalidStructure(error.to_string()))?
    };
    let row_matrix = if rows.is_empty() {
        None
    } else {
        let id =
            HeapId::new(4, 0).map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
        Some(NodeId::from(u32::from(id)))
    };

    let mut table = Vec::new();
    TableContextInfo::new(
        end_4byte,
        end_2byte,
        end_1byte,
        end_bitmap,
        row_index,
        row_matrix,
        columns.to_vec(),
    )
    .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
    .write(&mut table)?;

    let mut index = Vec::new();
    HeapTreeHeader::new(4, 4, 0, row_tree_root)
        .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
        .write(&mut index)?;
    let mut allocations = vec![table, index];
    if rows.is_empty() {
        return heap_page(HeapNodeType::Table, &allocations);
    }

    let mut leaf = Vec::with_capacity(rows.len().saturating_mul(8));
    for (index, row) in rows.iter().enumerate() {
        leaf.write_u32::<LittleEndian>(u32::from(row.id))?;
        leaf.write_u32::<LittleEndian>(
            u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("table row index"))?,
        )?;
    }

    let mut matrix = Vec::with_capacity(rows.len().saturating_mul(usize::from(end_bitmap)));
    let mut variable_values = Vec::new();
    for row in rows {
        let mut bytes = vec![0_u8; usize::from(end_bitmap)];
        bytes[0..4].copy_from_slice(&u32::from(row.id).to_le_bytes());
        bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
        mark_column(&mut bytes, columns, LTP_ROW_ID_PROP_ID)?;
        mark_column(&mut bytes, columns, LTP_ROW_VERSION_PROP_ID)?;
        for (property_id, value) in &row.values {
            write_table_value(
                &mut bytes,
                columns,
                *property_id,
                value,
                &mut variable_values,
            )?;
        }
        matrix.extend_from_slice(&bytes);
    }
    allocations.push(leaf);
    allocations.push(matrix);
    allocations.extend(variable_values);
    heap_page(HeapNodeType::Table, &allocations)
}

fn table_context_external(
    columns: &[TableColumnDescriptor],
    rows: &[TableRowSpec],
    next_block_index: &mut u64,
) -> Result<ExternalTableBuild, WriterError> {
    let mut rows = rows.iter().collect::<Vec<_>>();
    rows.sort_by_key(|row| u32::from(row.id));
    if rows.is_empty() {
        return Err(WriterError::InvalidStructure(
            "external table must contain rows".to_owned(),
        ));
    }

    let end_4byte = columns
        .iter()
        .filter(|column| {
            !matches!(
                column.prop_type(),
                PropertyType::Integer16 | PropertyType::Boolean
            )
        })
        .map(|column| column.offset().saturating_add(u16::from(column.size())))
        .max()
        .unwrap_or(0);
    let end_4byte = u16::try_from(align_up(u64::from(end_4byte), 4))
        .map_err(|_| WriterError::ValueTooLarge("table row"))?;
    let end_2byte = columns
        .iter()
        .filter(|column| column.prop_type() == PropertyType::Integer16)
        .map(|column| column.offset().saturating_add(2))
        .max()
        .unwrap_or(end_4byte)
        .max(end_4byte);
    let end_1byte = columns
        .iter()
        .filter(|column| column.prop_type() == PropertyType::Boolean)
        .map(|column| column.offset().saturating_add(1))
        .max()
        .unwrap_or(end_2byte)
        .max(end_2byte);
    let end_bitmap = end_1byte
        .checked_add(
            u16::try_from(columns.len().div_ceil(8))
                .map_err(|_| WriterError::ValueTooLarge("table bitmap"))?,
        )
        .ok_or(WriterError::ValueTooLarge("table row"))?;

    let row_index =
        HeapId::new(2, 0).map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
    let mut next_value_node = 0x5_0000_u32;
    let row_matrix_node = node(NodeIdType::ListsTablesProperties, next_value_node)?;
    next_value_node = next_value_node
        .checked_add(1)
        .ok_or(WriterError::ValueTooLarge("table value node"))?;

    let mut leaf = Vec::with_capacity(rows.len().saturating_mul(8));
    let mut matrix = Vec::with_capacity(rows.len().saturating_mul(usize::from(end_bitmap)));
    let mut blocks = Vec::new();
    let mut subnodes = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        leaf.write_u32::<LittleEndian>(u32::from(row.id))?;
        leaf.write_u32::<LittleEndian>(
            u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("table row index"))?,
        )?;

        let mut bytes = vec![0_u8; usize::from(end_bitmap)];
        bytes[0..4].copy_from_slice(&u32::from(row.id).to_le_bytes());
        bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
        mark_column(&mut bytes, columns, LTP_ROW_ID_PROP_ID)?;
        mark_column(&mut bytes, columns, LTP_ROW_VERSION_PROP_ID)?;
        for (property_id, value) in &row.values {
            write_external_table_value(
                &mut bytes,
                columns,
                *property_id,
                value,
                &mut next_value_node,
                next_block_index,
                &mut blocks,
                &mut subnodes,
            )?;
        }
        matrix.extend_from_slice(&bytes);
    }

    let matrix_root = append_row_matrix_data_tree(
        &matrix,
        usize::from(end_bitmap),
        next_block_index,
        &mut blocks,
    )?;
    subnodes.push(UnicodeLeafSubNodeTreeEntry::new(
        row_matrix_node,
        matrix_root,
        None,
    ));
    let subnode_block = append_subnode_tree(subnodes, next_block_index, &mut blocks)?;

    const BTH_RECORDS_PER_PAGE: usize = MAX_HEAP_ALLOCATION / 8;
    let mut heap_pages = Vec::new();
    let mut roots = Vec::with_capacity(rows.len().div_ceil(BTH_RECORDS_PER_PAGE));
    for chunk in leaf.chunks(BTH_RECORDS_PER_PAGE * 8) {
        let page_index = u16::try_from(heap_pages.len() + 1)
            .map_err(|_| WriterError::ValueTooLarge("heap page index"))?;
        let heap_id = HeapId::new(1, page_index)
            .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
        let key = u32::from_le_bytes(
            chunk[0..4]
                .try_into()
                .map_err(|_| WriterError::InvalidStructure("empty BTH leaf".to_owned()))?,
        );
        heap_pages.push(heap_continuation_page(page_index, chunk)?);
        roots.push((key, heap_id));
    }
    let mut levels = 0_u8;
    while roots.len() > 1 {
        levels = levels
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("BTH depth"))?;
        let mut parents = Vec::with_capacity(roots.len().div_ceil(BTH_RECORDS_PER_PAGE));
        for group in roots.chunks(BTH_RECORDS_PER_PAGE) {
            let mut entries = Vec::with_capacity(group.len() * 8);
            for (key, next_level) in group {
                entries.write_u32::<LittleEndian>(*key)?;
                entries.write_u32::<LittleEndian>(u32::from(*next_level))?;
            }
            let page_index = u16::try_from(heap_pages.len() + 1)
                .map_err(|_| WriterError::ValueTooLarge("heap page index"))?;
            let heap_id = HeapId::new(1, page_index)
                .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
            heap_pages.push(heap_continuation_page(page_index, &entries)?);
            parents.push((group[0].0, heap_id));
        }
        roots = parents;
    }
    let row_tree_root = roots[0].1;
    let mut table = Vec::new();
    TableContextInfo::new(
        end_4byte,
        end_2byte,
        end_1byte,
        end_bitmap,
        row_index,
        Some(row_matrix_node),
        columns.to_vec(),
    )
    .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
    .write(&mut table)?;
    let mut index = Vec::new();
    HeapTreeHeader::new(4, 4, levels, row_tree_root)
        .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
        .write(&mut index)?;
    heap_pages.insert(0, heap_page(HeapNodeType::Table, &[table, index])?);
    let non_final = heap_pages.len().saturating_sub(1);
    for page in heap_pages.iter_mut().take(non_final) {
        fill_heap_page(page)?;
    }
    update_heap_fill_levels(&mut heap_pages)?;
    let data_block = append_data_tree_pages(&heap_pages, next_block_index, &mut blocks)?;

    Ok(ExternalTableBuild {
        data_block,
        subnode_block,
        blocks,
    })
}

fn fill_heap_page(page: &mut Vec<u8>) -> Result<(), WriterError> {
    if page.len() >= MAX_DATA_BLOCK_PAYLOAD {
        if page.len() == MAX_DATA_BLOCK_PAYLOAD {
            return Ok(());
        }
        return Err(WriterError::ValueTooLarge("heap page"));
    }
    let offset_bytes = page
        .get(..2)
        .ok_or_else(|| WriterError::InvalidStructure("heap page has no map offset".to_owned()))?;
    let page_map_offset =
        usize::from(u16::from_le_bytes(offset_bytes.try_into().map_err(
            |_| WriterError::InvalidStructure("invalid heap map offset".to_owned()),
        )?));
    let mut page_map = page
        .get(page_map_offset..)
        .ok_or_else(|| WriterError::InvalidStructure("heap map exceeds its page".to_owned()))?
        .to_vec();
    let allocation_count = page_map
        .get(..2)
        .ok_or_else(|| WriterError::InvalidStructure("heap page map has no count".to_owned()))?;
    let allocation_count =
        u16::from_le_bytes(allocation_count.try_into().map_err(|_| {
            WriterError::InvalidStructure("invalid heap allocation count".to_owned())
        })?);
    let expected_map_size = usize::from(allocation_count)
        .checked_add(1)
        .and_then(|offset_count| offset_count.checked_mul(size_of::<u16>()))
        .and_then(|offsets_size| offsets_size.checked_add(2 * size_of::<u16>()))
        .ok_or(WriterError::ValueTooLarge("heap page map"))?;
    if page_map.len() != expected_map_size {
        return Err(WriterError::InvalidStructure(
            "heap page map size does not match its allocation count".to_owned(),
        ));
    }
    let allocation_end = usize::from(u16::from_le_bytes(
        page_map[page_map.len() - size_of::<u16>()..]
            .try_into()
            .map_err(|_| {
                WriterError::InvalidStructure("invalid heap allocation endpoint".to_owned())
            })?,
    ));
    let mut padding_allocations = 1_usize;
    let (filled_map_offset, padding_size) = loop {
        let expanded_map_size = padding_allocations
            .checked_mul(size_of::<u16>())
            .and_then(|padding_map_size| page_map.len().checked_add(padding_map_size))
            .ok_or(WriterError::ValueTooLarge("heap page map"))?;
        let filled_map_offset = MAX_DATA_BLOCK_PAYLOAD
            .checked_sub(expanded_map_size)
            .ok_or(WriterError::ValueTooLarge("heap page map"))?;
        let padding_size = filled_map_offset
            .checked_sub(allocation_end)
            .ok_or_else(|| {
                WriterError::InvalidStructure(
                    "heap page allocations overlap the filled map".to_owned(),
                )
            })?;
        let padding_capacity = padding_allocations
            .checked_mul(MAX_HEAP_ALLOCATION)
            .ok_or(WriterError::ValueTooLarge("heap padding allocations"))?;
        if padding_size < padding_allocations {
            return Err(WriterError::InvalidStructure(
                "heap page has no room for non-empty padding allocations".to_owned(),
            ));
        }
        if padding_size <= padding_capacity {
            break (filled_map_offset, padding_size);
        }
        padding_allocations = padding_allocations
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("heap padding allocations"))?;
    };
    let allocation_count = allocation_count
        .checked_add(
            u16::try_from(padding_allocations)
                .map_err(|_| WriterError::ValueTooLarge("heap allocation count"))?,
        )
        .ok_or(WriterError::ValueTooLarge("heap allocation count"))?;
    page_map[0..2].copy_from_slice(&allocation_count.to_le_bytes());
    let mut padding_end = allocation_end;
    let mut padding_remaining = padding_size;
    for allocation_index in 0..padding_allocations {
        let allocations_remaining = padding_allocations - allocation_index;
        let allocation_size = padding_remaining
            .saturating_sub(allocations_remaining - 1)
            .min(MAX_HEAP_ALLOCATION);
        padding_end = padding_end
            .checked_add(allocation_size)
            .ok_or(WriterError::ValueTooLarge("heap padding allocation"))?;
        page_map.extend_from_slice(
            &u16::try_from(padding_end)
                .map_err(|_| WriterError::ValueTooLarge("heap padding endpoint"))?
                .to_le_bytes(),
        );
        padding_remaining -= allocation_size;
    }
    if padding_remaining != 0 || padding_end != filled_map_offset {
        return Err(WriterError::InvalidStructure(
            "heap padding does not reach the page map".to_owned(),
        ));
    }
    let filled_map_offset = u16::try_from(filled_map_offset)
        .map_err(|_| WriterError::ValueTooLarge("heap page map offset"))?;
    page.truncate(page_map_offset);
    page.resize(usize::from(filled_map_offset), 0);
    page.extend_from_slice(&page_map);
    page[0..2].copy_from_slice(&filled_map_offset.to_le_bytes());
    Ok(())
}

fn update_heap_fill_levels(pages: &mut [Vec<u8>]) -> Result<(), WriterError> {
    let levels = pages
        .iter()
        .map(|page| heap_fill_level(page.len()))
        .collect::<Result<Vec<_>, _>>()?;
    let root_page = pages
        .first_mut()
        .ok_or_else(|| WriterError::InvalidStructure("heap has no root page".to_owned()))?;
    let root_header = HeapNodeHeader::read(&mut io::Cursor::new(root_page.as_slice()))?;
    let mut root_levels = [HeapFillLevel::Empty; 8];
    let root_count = levels.len().min(root_levels.len());
    root_levels[..root_count].copy_from_slice(&levels[..root_count]);
    HeapNodeHeader::new(
        root_header.page_map_offset(),
        root_header.client_signature(),
        root_header.user_root(),
        root_levels,
    )
    .write(&mut io::Cursor::new(root_page.as_mut_slice()))?;

    for bitmap_index in (8..pages.len()).step_by(128) {
        let bitmap_page = pages.get_mut(bitmap_index).ok_or_else(|| {
            WriterError::InvalidStructure("heap bitmap page is missing".to_owned())
        })?;
        let bitmap_header =
            HeapNodeBitmapHeader::read(&mut io::Cursor::new(bitmap_page.as_slice()))?;
        let mut bitmap_levels = [HeapFillLevel::Empty; 128];
        let represented_count = levels.len().saturating_sub(bitmap_index).min(128);
        bitmap_levels[..represented_count]
            .copy_from_slice(&levels[bitmap_index..bitmap_index + represented_count]);
        HeapNodeBitmapHeader::new(bitmap_header.page_map_offset(), bitmap_levels)
            .write(&mut io::Cursor::new(bitmap_page.as_mut_slice()))?;
    }
    Ok(())
}

fn heap_fill_level(page_size: usize) -> Result<HeapFillLevel, WriterError> {
    let free = MAX_DATA_BLOCK_PAYLOAD
        .checked_sub(page_size)
        .ok_or(WriterError::ValueTooLarge("heap page"))?;
    Ok(match free {
        3584.. => HeapFillLevel::Empty,
        2560..=3583 => HeapFillLevel::Level1,
        2048..=2559 => HeapFillLevel::Level2,
        1792..=2047 => HeapFillLevel::Level3,
        1536..=1791 => HeapFillLevel::Level4,
        1280..=1535 => HeapFillLevel::Level5,
        1024..=1279 => HeapFillLevel::Level6,
        768..=1023 => HeapFillLevel::Level7,
        512..=767 => HeapFillLevel::Level8,
        256..=511 => HeapFillLevel::Level9,
        128..=255 => HeapFillLevel::Level10,
        64..=127 => HeapFillLevel::Level11,
        32..=63 => HeapFillLevel::Level12,
        16..=31 => HeapFillLevel::Level13,
        8..=15 => HeapFillLevel::Level14,
        0..=7 => HeapFillLevel::Level15,
    })
}

#[allow(clippy::too_many_arguments)]
fn write_external_table_value(
    row: &mut [u8],
    columns: &[TableColumnDescriptor],
    property_id: u16,
    value: &PropertyValue,
    next_value_node: &mut u32,
    next_block_index: &mut u64,
    blocks: &mut Vec<BlockSpec>,
    subnodes: &mut Vec<UnicodeLeafSubNodeTreeEntry>,
) -> Result<(), WriterError> {
    let column = columns
        .iter()
        .find(|column| column.prop_id() == property_id)
        .ok_or_else(|| WriterError::InvalidStructure("table value has no column".to_owned()))?;
    let offset = usize::from(column.offset());
    match value {
        PropertyValue::Integer16(value) => write_row_bytes(row, offset, &value.to_le_bytes())?,
        PropertyValue::Integer32(value) => write_row_bytes(row, offset, &value.to_le_bytes())?,
        PropertyValue::Floating32(value) | PropertyValue::ErrorCode(value) => {
            write_row_bytes(row, offset, &value.to_le_bytes())?
        }
        PropertyValue::Boolean(value) => write_row_bytes(row, offset, &[u8::from(*value)])?,
        PropertyValue::Integer64(value)
        | PropertyValue::Currency(value)
        | PropertyValue::Time(value) => write_row_bytes(row, offset, &value.to_le_bytes())?,
        PropertyValue::Floating64(value) | PropertyValue::FloatingTime(value) => {
            write_row_bytes(row, offset, &value.to_le_bytes())?
        }
        PropertyValue::Guid(_)
        | PropertyValue::Unicode(_)
        | PropertyValue::Binary(_)
        | PropertyValue::MultipleInteger16(_)
        | PropertyValue::MultipleInteger32(_)
        | PropertyValue::MultipleInteger64(_)
        | PropertyValue::MultipleGuid(_)
        | PropertyValue::Object(_, _) => {
            let data = table_variable_bytes(value)?.ok_or_else(|| {
                WriterError::InvalidStructure("table variable value is missing".to_owned())
            })?;
            if data.is_empty() {
                write_row_bytes(row, offset, &0_u32.to_le_bytes())?;
                return mark_column(row, columns, property_id);
            }
            let value_node = node(NodeIdType::ListsTablesProperties, *next_value_node)?;
            *next_value_node = next_value_node
                .checked_add(1)
                .ok_or(WriterError::ValueTooLarge("table value node"))?;
            let root = append_data_tree(&data, next_block_index, blocks)?;
            subnodes.push(UnicodeLeafSubNodeTreeEntry::new(value_node, root, None));
            write_row_bytes(row, offset, &u32::from(value_node).to_le_bytes())?;
        }
        PropertyValue::External(_, _) => {
            return Err(WriterError::InvalidStructure(
                "table values cannot reference property subnodes".to_owned(),
            ));
        }
    }
    mark_column(row, columns, property_id)
}

fn write_table_value(
    row: &mut [u8],
    columns: &[TableColumnDescriptor],
    property_id: u16,
    value: &PropertyValue,
    variables: &mut Vec<Vec<u8>>,
) -> Result<(), WriterError> {
    let column = columns
        .iter()
        .find(|column| column.prop_id() == property_id)
        .ok_or_else(|| WriterError::InvalidStructure("table value has no column".to_owned()))?;
    let offset = usize::from(column.offset());
    match value {
        PropertyValue::Integer16(value) => write_row_bytes(row, offset, &value.to_le_bytes())?,
        PropertyValue::Integer32(value) => write_row_bytes(row, offset, &value.to_le_bytes())?,
        PropertyValue::Floating32(value) | PropertyValue::ErrorCode(value) => {
            write_row_bytes(row, offset, &value.to_le_bytes())?
        }
        PropertyValue::Boolean(value) => write_row_bytes(row, offset, &[u8::from(*value)])?,
        PropertyValue::Integer64(value)
        | PropertyValue::Currency(value)
        | PropertyValue::Time(value) => write_row_bytes(row, offset, &value.to_le_bytes())?,
        PropertyValue::Floating64(value) | PropertyValue::FloatingTime(value) => {
            write_row_bytes(row, offset, &value.to_le_bytes())?
        }
        PropertyValue::Guid(_)
        | PropertyValue::Unicode(_)
        | PropertyValue::Binary(_)
        | PropertyValue::MultipleInteger16(_)
        | PropertyValue::MultipleInteger32(_)
        | PropertyValue::MultipleInteger64(_)
        | PropertyValue::MultipleGuid(_)
        | PropertyValue::Object(_, _) => {
            let data = table_variable_bytes(value)?.ok_or_else(|| {
                WriterError::InvalidStructure("table variable value is missing".to_owned())
            })?;
            if data.is_empty() {
                write_row_bytes(row, offset, &0_u32.to_le_bytes())?;
                return mark_column(row, columns, property_id);
            }
            variables.push(data);
            let allocation = 4_usize
                .checked_add(variables.len())
                .ok_or(WriterError::ValueTooLarge("table allocation"))?;
            let heap_id = HeapId::new(
                u16::try_from(allocation)
                    .map_err(|_| WriterError::ValueTooLarge("table allocation"))?,
                0,
            )
            .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
            write_row_bytes(row, offset, &u32::from(heap_id).to_le_bytes())?;
        }
        PropertyValue::External(_, _) => {
            return Err(WriterError::InvalidStructure(
                "table values cannot reference property subnodes".to_owned(),
            ));
        }
    }
    mark_column(row, columns, property_id)
}

fn table_variable_bytes(value: &PropertyValue) -> io::Result<Option<Vec<u8>>> {
    let data = match value {
        PropertyValue::Unicode(value) => unicode_bytes(value)?,
        PropertyValue::Binary(value) => value.clone(),
        PropertyValue::Object(node, size) => {
            [u32::from(*node).to_le_bytes(), size.to_le_bytes()].concat()
        }
        _ => return value.variable_bytes(),
    };
    Ok(Some(data))
}

fn unicode_bytes(value: &str) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(value.len().saturating_mul(2));
    for unit in value.encode_utf16() {
        bytes.write_u16::<LittleEndian>(unit)?;
    }
    Ok(bytes)
}

fn write_row_bytes(row: &mut [u8], offset: usize, value: &[u8]) -> Result<(), WriterError> {
    let end = offset
        .checked_add(value.len())
        .ok_or(WriterError::ValueTooLarge("table row offset"))?;
    let target = row
        .get_mut(offset..end)
        .ok_or_else(|| WriterError::InvalidStructure("table value exceeds row".to_owned()))?;
    target.copy_from_slice(value);
    Ok(())
}

fn mark_column(
    row: &mut [u8],
    columns: &[TableColumnDescriptor],
    property_id: u16,
) -> Result<(), WriterError> {
    let column = columns
        .iter()
        .find(|column| column.prop_id() == property_id)
        .ok_or_else(|| WriterError::InvalidStructure("table value has no column".to_owned()))?;
    let bitmap_size = columns.len().div_ceil(8);
    let bitmap_start = row
        .len()
        .checked_sub(bitmap_size)
        .ok_or_else(|| WriterError::InvalidStructure("table bitmap underflow".to_owned()))?;
    let bit = usize::from(column.existence_bitmap_index());
    let byte = row
        .get_mut(bitmap_start + bit / 8)
        .ok_or_else(|| WriterError::InvalidStructure("table bitmap overflow".to_owned()))?;
    *byte |= 0x80_u8 >> (bit % 8);
    Ok(())
}

fn heap_page(kind: HeapNodeType, allocations: &[Vec<u8>]) -> Result<Vec<u8>, WriterError> {
    let header_size = 12_usize;
    let allocation_end = allocations
        .iter()
        .try_fold(header_size, |size, allocation| {
            size.checked_add(allocation.len())
        })
        .ok_or(WriterError::ValueTooLarge("heap page"))?;
    let payload_size = align_up(
        u64::try_from(allocation_end).map_err(|_| WriterError::ValueTooLarge("heap page"))?,
        2,
    );
    let payload_size =
        usize::try_from(payload_size).map_err(|_| WriterError::ValueTooLarge("heap page"))?;
    let page_map_size = 4_usize
        .checked_add(
            allocations
                .len()
                .checked_add(1)
                .and_then(|count| count.checked_mul(2))
                .ok_or(WriterError::ValueTooLarge("heap page map"))?,
        )
        .ok_or(WriterError::ValueTooLarge("heap page map"))?;
    let total = payload_size
        .checked_add(page_map_size)
        .ok_or(WriterError::ValueTooLarge("heap page"))?;
    if total > 8176 {
        return Err(WriterError::ValueTooLarge("heap page"));
    }

    let page_map_offset =
        u16::try_from(payload_size).map_err(|_| WriterError::ValueTooLarge("heap page offset"))?;
    let user_root =
        HeapId::new(1, 0).map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
    let header = HeapNodeHeader::new(page_map_offset, kind, user_root, [HeapFillLevel::Empty; 8]);

    let mut data = Vec::with_capacity(total);
    header.write(&mut data)?;
    let mut offsets = Vec::with_capacity(allocations.len() + 1);
    offsets.push(u16::try_from(data.len()).map_err(|_| WriterError::ValueTooLarge("heap"))?);
    for allocation in allocations {
        data.extend_from_slice(allocation);
        offsets.push(u16::try_from(data.len()).map_err(|_| WriterError::ValueTooLarge("heap"))?);
    }
    data.resize(payload_size, 0);
    data.write_u16::<LittleEndian>(
        u16::try_from(allocations.len())
            .map_err(|_| WriterError::ValueTooLarge("heap allocation count"))?,
    )?;
    let free_count = allocations
        .iter()
        .filter(|allocation| allocation.is_empty())
        .count();
    data.write_u16::<LittleEndian>(
        u16::try_from(free_count)
            .map_err(|_| WriterError::ValueTooLarge("heap free allocation count"))?,
    )?;
    for offset in offsets {
        data.write_u16::<LittleEndian>(offset)?;
    }
    Ok(data)
}

fn heap_continuation_page(page_index: u16, allocation: &[u8]) -> Result<Vec<u8>, WriterError> {
    let bitmap = page_index % 128 == 8;
    let header_size = if bitmap { 66_usize } else { 2_usize };
    let allocation_end = header_size
        .checked_add(allocation.len())
        .ok_or(WriterError::ValueTooLarge("heap continuation page"))?;
    let payload_size = usize::try_from(align_up(
        u64::try_from(allocation_end)
            .map_err(|_| WriterError::ValueTooLarge("heap continuation page"))?,
        2,
    ))
    .map_err(|_| WriterError::ValueTooLarge("heap continuation page"))?;
    let total = payload_size
        .checked_add(8)
        .ok_or(WriterError::ValueTooLarge("heap continuation page"))?;
    if total > MAX_DATA_BLOCK_PAYLOAD {
        return Err(WriterError::ValueTooLarge("heap continuation page"));
    }
    let page_map_offset = u16::try_from(payload_size)
        .map_err(|_| WriterError::ValueTooLarge("heap continuation page"))?;
    let mut data = Vec::with_capacity(total);
    if bitmap {
        HeapNodeBitmapHeader::new(page_map_offset, [HeapFillLevel::Empty; 128]).write(&mut data)?;
    } else {
        HeapNodePageHeader::new(page_map_offset).write(&mut data)?;
    }
    data.extend_from_slice(allocation);
    data.resize(payload_size, 0);
    data.write_u16::<LittleEndian>(1)?;
    data.write_u16::<LittleEndian>(0)?;
    data.write_u16::<LittleEndian>(
        u16::try_from(header_size)
            .map_err(|_| WriterError::ValueTooLarge("heap continuation offset"))?,
    )?;
    data.write_u16::<LittleEndian>(
        u16::try_from(allocation_end)
            .map_err(|_| WriterError::ValueTooLarge("heap continuation offset"))?,
    )?;
    Ok(data)
}

fn write_blocks(
    file: &mut std::fs::File,
    blocks: &[BlockSpec],
    cursor: &mut u64,
    interrupted: &AtomicBool,
) -> Result<Vec<WrittenBlock>, WriterError> {
    let mut written = Vec::with_capacity(blocks.len());
    for block in blocks {
        check_interrupted(interrupted)?;
        let size = u16::try_from(block.payload.logical_size())
            .map_err(|_| WriterError::ValueTooLarge("data block"))?;
        let physical_size = u64::from(block_size(size.saturating_add(16))?);
        let offset = allocate_extent(cursor, physical_size, SLOT_SIZE)?;
        file.seek(SeekFrom::Start(offset))?;
        let signature = compute_sig(offset as u32, u64::from(block.id) as u32);
        let trailer = UnicodeBlockTrailer::new(size, signature, 0, block.id)
            .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
        match &block.payload {
            BlockPayload::Subnode(entries) => {
                UnicodeLeafSubNodeTreeBlock::new(
                    UnicodeSubNodeTreeBlockHeader::new(
                        0,
                        u16::try_from(entries.len())
                            .map_err(|_| WriterError::ValueTooLarge("subnode entry count"))?,
                    ),
                    entries.clone(),
                    trailer,
                )
                .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
                .write(file)?;
            }
            BlockPayload::DataTree {
                level,
                total_size,
                entries,
            } => {
                UnicodeDataTreeBlock::new(
                    DataTreeBlockHeader::new(
                        *level,
                        u16::try_from(entries.len())
                            .map_err(|_| WriterError::ValueTooLarge("data-tree entry count"))?,
                        *total_size,
                    ),
                    entries.clone(),
                    trailer,
                )
                .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
                .write(file)?;
            }
            BlockPayload::Data(data) => {
                UnicodeDataBlock::new(NdbCryptMethod::Permute, data.clone(), trailer)
                    .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
                    .write(file)?;
            }
            BlockPayload::IntermediateSubnode { level, entries } => {
                UnicodeIntermediateSubNodeTreeBlock::new(
                    UnicodeSubNodeTreeBlockHeader::new(
                        *level,
                        u16::try_from(entries.len())
                            .map_err(|_| WriterError::ValueTooLarge("subnode entry count"))?,
                    ),
                    entries.clone(),
                    trailer,
                )
                .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
                .write(file)?;
            }
        }
        written.push(WrittenBlock {
            id: block.id,
            offset,
            size,
            ref_count: block.ref_count,
        });
    }
    Ok(written)
}

fn write_bbt(
    file: &mut std::fs::File,
    first_offset: u64,
    first_page_id: u64,
    blocks: &[WrittenBlock],
) -> Result<(UnicodePageRef, u64, u64), WriterError> {
    let pages = plan_leaf_pages(blocks.len(), 20)?;
    if pages.is_empty() {
        return Err(WriterError::InvalidStructure("BBT is empty".to_owned()));
    }
    let entries = blocks
        .iter()
        .map(|block| {
            UnicodeBlockBTreeEntry::new_with_ref_count(
                UnicodeBlockRef::new(block.id, UnicodeByteIndex::new(block.offset)),
                block.size,
                block.ref_count,
            )
        })
        .collect::<Vec<_>>();
    let mut roots = Vec::with_capacity(pages.len());
    let mut next_offset = first_offset;
    let mut next_page = first_page_id;
    for range in &pages {
        let offset = allocate_extent(&mut next_offset, PAGE_SIZE, PAGE_SIZE)?;
        let page_id = UnicodePageId::from(next_page);
        let page = UnicodeBlockBTreePage::new(
            0,
            20,
            24,
            &entries[range.clone()],
            page_trailer(PageType::BlockBTree, offset, page_id),
        )
        .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
        file.seek(SeekFrom::Start(offset))?;
        page.write(file)?;
        roots.push(<UnicodeBTreePageEntry as BTreePageEntryReadWrite>::new(
            entries[range.start].key(),
            UnicodePageRef::new(page_id, UnicodeByteIndex::new(offset)),
        ));
        next_page = next_page
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("BBT page id"))?;
    }
    let mut level = 1_u8;
    while roots.len() > 1 {
        let ranges = plan_leaf_pages(roots.len(), 20)?;
        let mut parents = Vec::with_capacity(ranges.len());
        for range in ranges {
            let offset = allocate_extent(&mut next_offset, PAGE_SIZE, PAGE_SIZE)?;
            let page_id = UnicodePageId::from(next_page);
            let page = UnicodeBTreeEntryPage::new(
                level,
                20,
                24,
                &roots[range.clone()],
                page_trailer(PageType::BlockBTree, offset, page_id),
            )
            .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
            file.seek(SeekFrom::Start(offset))?;
            page.write(file)?;
            parents.push(<UnicodeBTreePageEntry as BTreePageEntryReadWrite>::new(
                roots[range.start].key(),
                UnicodePageRef::new(page_id, UnicodeByteIndex::new(offset)),
            ));
            next_page = next_page
                .checked_add(1)
                .ok_or(WriterError::ValueTooLarge("BBT page id"))?;
        }
        roots = parents;
        level = level
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("BBT depth"))?;
    }
    Ok((roots[0].block(), next_offset, next_page))
}

fn write_nbt(
    file: &mut std::fs::File,
    first_offset: u64,
    first_page_id: u64,
    entries: &[UnicodeNodeBTreeEntry],
) -> Result<(UnicodePageRef, u64, u64), WriterError> {
    let pages = plan_leaf_pages(entries.len(), 15)?;
    if pages.is_empty() {
        return Err(WriterError::InvalidStructure("NBT is empty".to_owned()));
    }
    let mut roots = Vec::with_capacity(pages.len());
    let mut next_offset = first_offset;
    let mut next_page = first_page_id;
    for range in &pages {
        let offset = allocate_extent(&mut next_offset, PAGE_SIZE, PAGE_SIZE)?;
        let page_id = UnicodePageId::from(next_page);
        let trailer = page_trailer(PageType::NodeBTree, offset, page_id);
        let page = UnicodeNodeBTreePage::new(0, 15, 32, &entries[range.clone()], trailer)
            .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
        file.seek(SeekFrom::Start(offset))?;
        page.write(file)?;
        roots.push(<UnicodeBTreePageEntry as BTreePageEntryReadWrite>::new(
            entries[range.start].key(),
            UnicodePageRef::new(page_id, UnicodeByteIndex::new(offset)),
        ));
        next_page = next_page
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("NBT page id"))?;
    }
    let mut level = 1_u8;
    while roots.len() > 1 {
        let ranges = plan_leaf_pages(roots.len(), 20)?;
        let mut parents = Vec::with_capacity(ranges.len());
        for range in ranges {
            let offset = allocate_extent(&mut next_offset, PAGE_SIZE, PAGE_SIZE)?;
            let page_id = UnicodePageId::from(next_page);
            let page = UnicodeBTreeEntryPage::new(
                level,
                20,
                24,
                &roots[range.clone()],
                page_trailer(PageType::NodeBTree, offset, page_id),
            )
            .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
            file.seek(SeekFrom::Start(offset))?;
            page.write(file)?;
            parents.push(<UnicodeBTreePageEntry as BTreePageEntryReadWrite>::new(
                roots[range.start].key(),
                UnicodePageRef::new(page_id, UnicodeByteIndex::new(offset)),
            ));
            next_page = next_page
                .checked_add(1)
                .ok_or(WriterError::ValueTooLarge("NBT page id"))?;
        }
        roots = parents;
        level = level
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("NBT depth"))?;
    }
    Ok((roots[0].block(), next_offset, next_page))
}

struct TopMessageNode {
    node: NodeId,
    property_block: UnicodeBlockId,
    subnode_block: UnicodeBlockId,
    parent: NodeId,
}

struct TopFolderNode {
    node: NodeId,
    parent: NodeId,
    property_block: UnicodeBlockId,
    hierarchy_block: UnicodeBlockId,
    contents_block: UnicodeBlockId,
    contents_subnode: Option<UnicodeBlockId>,
    associated_block: UnicodeBlockId,
    associated_subnode: Option<UnicodeBlockId>,
}

fn node_entries(
    root: NodeId,
    ipm: NodeId,
    search_root: NodeId,
    deleted: NodeId,
    spam_search: NodeId,
    folders: &[TopFolderNode],
    messages: &[TopMessageNode],
) -> Result<Vec<UnicodeNodeBTreeEntry>, WriterError> {
    let deleted_override = folders.iter().find(|folder| folder.node == deleted);
    let deleted_property = deleted_override.map_or(leaf_bid(8)?, |folder| folder.property_block);
    let deleted_hierarchy = deleted_override.map_or(leaf_bid(9)?, |folder| folder.hierarchy_block);
    let deleted_contents = deleted_override.map_or(leaf_bid(5)?, |folder| folder.contents_block);
    let deleted_contents_subnode = deleted_override.and_then(|folder| folder.contents_subnode);
    let deleted_associated =
        deleted_override.map_or(leaf_bid(13)?, |folder| folder.associated_block);
    let deleted_associated_subnode = deleted_override.and_then(|folder| folder.associated_subnode);
    let mut entries = vec![
        UnicodeNodeBTreeEntry::new(NID_MESSAGE_STORE, leaf_bid(1)?, None, None),
        UnicodeNodeBTreeEntry::new(NID_NAME_TO_ID_MAP, leaf_bid(2)?, None, None),
        UnicodeNodeBTreeEntry::new(
            NID_SEARCH_MANAGEMENT_QUEUE,
            UnicodeBlockId::default(),
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(NID_SEARCH_ACTIVITY_LIST, leaf_bid(19)?, None, None),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_RECEIVE_FOLDER_TABLE),
            leaf_bid(20)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_OUTGOING_QUEUE_TABLE),
            leaf_bid(21)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_HIERARCHY_TABLE_TEMPLATE),
            leaf_bid(9)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_CONTENTS_TABLE_TEMPLATE),
            leaf_bid(5)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_ASSOC_CONTENTS_TABLE_TEMPLATE),
            leaf_bid(13)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_SEARCH_CONTENTS_TABLE_TEMPLATE),
            leaf_bid(16)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_RECIPIENT_TABLE_TEMPLATE),
            leaf_bid(17)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_ATTACHMENT_TABLE_TEMPLATE),
            leaf_bid(18)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_CONTENTS_INDEX_TEMPLATE),
            leaf_bid(22)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_SEARCH_INDEX_TEMPLATE),
            leaf_bid(23)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_ATTACHMENT_INDEX_TEMPLATE),
            leaf_bid(24)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(NodeId::from(NID_HIERARCHY_MAP), leaf_bid(26)?, None, None),
        UnicodeNodeBTreeEntry::new(
            NodeId::from(NID_SEARCH_FOLDER_TEMPLATE),
            UnicodeBlockId::default(),
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(root, leaf_bid(3)?, None, Some(root)),
        table_node(root, NodeIdType::HierarchyTable, leaf_bid(4)?)?,
        table_node(root, NodeIdType::ContentsTable, leaf_bid(5)?)?,
        table_node(root, NodeIdType::AssociatedContentsTable, leaf_bid(13)?)?,
        UnicodeNodeBTreeEntry::new(ipm, leaf_bid(6)?, None, Some(root)),
        table_node(ipm, NodeIdType::HierarchyTable, leaf_bid(7)?)?,
        table_node(ipm, NodeIdType::ContentsTable, leaf_bid(5)?)?,
        table_node(ipm, NodeIdType::AssociatedContentsTable, leaf_bid(13)?)?,
        UnicodeNodeBTreeEntry::new(search_root, leaf_bid(14)?, None, Some(root)),
        table_node(search_root, NodeIdType::HierarchyTable, leaf_bid(9)?)?,
        table_node(search_root, NodeIdType::ContentsTable, leaf_bid(5)?)?,
        table_node(
            search_root,
            NodeIdType::AssociatedContentsTable,
            leaf_bid(13)?,
        )?,
        UnicodeNodeBTreeEntry::new(deleted, deleted_property, None, Some(ipm)),
        table_node(deleted, NodeIdType::HierarchyTable, deleted_hierarchy)?,
        table_node_with_subnode(
            deleted,
            NodeIdType::ContentsTable,
            deleted_contents,
            deleted_contents_subnode,
        )?,
        table_node_with_subnode(
            deleted,
            NodeIdType::AssociatedContentsTable,
            deleted_associated,
            deleted_associated_subnode,
        )?,
        UnicodeNodeBTreeEntry::new(spam_search, leaf_bid(15)?, None, Some(root)),
        UnicodeNodeBTreeEntry::new(
            node(NodeIdType::SearchUpdateQueue, SPAM_SEARCH_INDEX)?,
            UnicodeBlockId::default(),
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            node(NodeIdType::SearchCriteria, SPAM_SEARCH_INDEX)?,
            leaf_bid(25)?,
            None,
            None,
        ),
        UnicodeNodeBTreeEntry::new(
            node(NodeIdType::SearchContentsTable, SPAM_SEARCH_INDEX)?,
            leaf_bid(16)?,
            None,
            None,
        ),
    ];
    for folder in folders.iter().filter(|folder| folder.node != deleted) {
        entries.extend([
            UnicodeNodeBTreeEntry::new(
                folder.node,
                folder.property_block,
                None,
                Some(folder.parent),
            ),
            table_node_with_subnode(
                folder.node,
                NodeIdType::HierarchyTable,
                folder.hierarchy_block,
                None,
            )?,
            table_node_with_subnode(
                folder.node,
                NodeIdType::ContentsTable,
                folder.contents_block,
                folder.contents_subnode,
            )?,
            table_node_with_subnode(
                folder.node,
                NodeIdType::AssociatedContentsTable,
                folder.associated_block,
                folder.associated_subnode,
            )?,
        ]);
    }
    entries.extend(messages.iter().map(|message| {
        UnicodeNodeBTreeEntry::new(
            message.node,
            message.property_block,
            Some(message.subnode_block),
            Some(message.parent),
        )
    }));
    entries.sort_by_key(|entry| entry.key());
    Ok(entries)
}

fn table_node(
    folder: NodeId,
    kind: NodeIdType,
    data: UnicodeBlockId,
) -> Result<UnicodeNodeBTreeEntry, WriterError> {
    table_node_with_subnode(folder, kind, data, None)
}

fn table_node_with_subnode(
    folder: NodeId,
    kind: NodeIdType,
    data: UnicodeBlockId,
    subnode: Option<UnicodeBlockId>,
) -> Result<UnicodeNodeBTreeEntry, WriterError> {
    Ok(UnicodeNodeBTreeEntry::new(
        node(kind, folder.index())?,
        data,
        subnode,
        None,
    ))
}

fn write_fixed_pages(
    file: &mut std::fs::File,
    allocated_end: u64,
    next_page_id: UnicodePageId,
) -> Result<(), WriterError> {
    if allocated_end > INITIAL_FILE_EOF {
        return Err(WriterError::ValueTooLarge("initial allocation region"));
    }
    let allocated_slots = allocated_end
        .checked_sub(FIRST_AMAP)
        .ok_or_else(|| WriterError::InvalidStructure("allocation start underflow".to_owned()))?
        .div_ceil(SLOT_SIZE);
    let mut amap_bits = [0_u8; 496];
    for slot in 0..allocated_slots {
        let byte = usize::try_from(slot / 8)
            .map_err(|_| WriterError::ValueTooLarge("allocation map index"))?;
        let bit =
            u8::try_from(slot % 8).map_err(|_| WriterError::ValueTooLarge("allocation map bit"))?;
        amap_bits[byte] |= 0x80_u8 >> bit;
    }

    let density_trailer = page_trailer(
        PageType::DensityList,
        DENSITY_LIST_FILE_OFFSET,
        next_page_id,
    );
    let free_slots = SLOTS_PER_AMAP
        .checked_sub(allocated_slots)
        .ok_or(WriterError::ValueTooLarge("density list free slots"))?;
    let density_entry = DensityListPageEntry::new(
        0,
        u16::try_from(free_slots)
            .map_err(|_| WriterError::ValueTooLarge("density list free slots"))?,
    )
    .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
    UnicodeDensityListPage::new(true, 1, &[density_entry], density_trailer)
        .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
        .write(file)?;

    file.seek(SeekFrom::Start(FIRST_AMAP))?;
    let amap_trailer = page_trailer(
        PageType::AllocationMap,
        FIRST_AMAP,
        UnicodePageId::from(FIRST_AMAP),
    );
    let amap = <UnicodeMapPage<{ PageType::AllocationMap as u8 }> as MapPageReadWrite<
        crate::UnicodePstFile,
        { PageType::AllocationMap as u8 },
    >>::new(amap_bits, amap_trailer)
    .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
    MapPageReadWrite::write(&amap, file)?;

    file.seek(SeekFrom::Start(FIRST_PMAP))?;
    let pmap_trailer = page_trailer(
        PageType::AllocationPageMap,
        FIRST_PMAP,
        UnicodePageId::from(FIRST_PMAP),
    );
    let pmap = <UnicodeMapPage<{ PageType::AllocationPageMap as u8 }> as MapPageReadWrite<
        crate::UnicodePstFile,
        { PageType::AllocationPageMap as u8 },
    >>::new([0xFF; 496], pmap_trailer)
    .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
    MapPageReadWrite::write(&pmap, file)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_header(
    file: &mut std::fs::File,
    nbt: UnicodePageRef,
    bbt: UnicodePageRef,
    allocated_end: u64,
    next_page_id: UnicodePageId,
    next_block_id: UnicodeBlockId,
    nids: [u32; 32],
    dynamic_allocation: bool,
) -> Result<(), WriterError> {
    let file_eof = allocation_file_eof(allocated_end)?;
    let free_bytes = if dynamic_allocation {
        0
    } else {
        let allocated_slots = allocated_end
            .checked_sub(FIRST_AMAP)
            .ok_or_else(|| WriterError::InvalidStructure("allocation start underflow".to_owned()))?
            .div_ceil(SLOT_SIZE);
        SLOTS_PER_AMAP
            .checked_sub(allocated_slots)
            .and_then(|slots| slots.checked_mul(SLOT_SIZE))
            .ok_or(WriterError::ValueTooLarge("free byte count"))?
    };
    let root = UnicodeRoot::new(
        UnicodeByteIndex::new(file_eof),
        UnicodeByteIndex::new(file_eof - AMAP_DATA_SIZE),
        UnicodeByteIndex::new(free_bytes),
        UnicodeByteIndex::new(0),
        nbt,
        bbt,
        if dynamic_allocation {
            AmapStatus::Invalid
        } else {
            AmapStatus::Valid2
        },
    );
    let header = UnicodeHeader::new_store(
        root,
        NdbCryptMethod::Permute,
        next_page_id,
        next_block_id,
        2,
        nids,
    );
    file.seek(SeekFrom::Start(0))?;
    header.write(file)?;
    Ok(())
}

fn nid_counters(
    entries: &[UnicodeNodeBTreeEntry],
    blocks: &[BlockSpec],
    streamed_subnodes: &[NodeId],
) -> Result<[u32; 32], WriterError> {
    let mut counters = INITIAL_NID_COUNTERS;
    for entry in entries {
        update_nid_counter(&mut counters, entry.node(), true)?;
    }
    for block in blocks {
        let BlockPayload::Subnode(entries) = &block.payload else {
            continue;
        };
        for entry in entries {
            update_nid_counter(&mut counters, entry.node(), false)?;
        }
    }
    for node in streamed_subnodes {
        update_nid_counter(&mut counters, *node, false)?;
    }
    Ok(counters)
}

fn update_nid_counter(
    counters: &mut [u32; 32],
    node: NodeId,
    top_level: bool,
) -> Result<(), WriterError> {
    let kind = match node.id_type() {
        Ok(kind) => kind,
        // Outlook's 0x6B6 persisted-view template uses a reserved type
        // value and therefore has no creation counter.
        Err(_)
            if matches!(
                u32::from(node),
                NID_CONTENTS_INDEX_TEMPLATE
                    | NID_SEARCH_INDEX_TEMPLATE
                    | NID_ATTACHMENT_INDEX_TEMPLATE
            ) =>
        {
            return Ok(());
        }
        Err(error) => return Err(WriterError::InvalidStructure(error.to_string())),
    };
    let index =
        usize::try_from(kind).map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
    let high_water = if top_level {
        node.index()
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("node counter"))?
    } else {
        node.index()
    };
    counters[index] = counters[index].max(high_water);
    Ok(())
}

fn entry_id(record_key: [u8; 16], node_id: NodeId) -> Result<Vec<u8>, WriterError> {
    let mut bytes = Vec::with_capacity(24);
    bytes.write_u32::<LittleEndian>(0)?;
    bytes.extend_from_slice(&record_key);
    bytes.write_u32::<LittleEndian>(u32::from(node_id))?;
    Ok(bytes)
}

fn message_instance_entry_id(record_key: [u8; 16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&record_key);
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes
}

fn node(kind: NodeIdType, index: u32) -> Result<NodeId, WriterError> {
    NodeId::new(kind, index).map_err(|error| WriterError::InvalidStructure(error.to_string()))
}

fn leaf_bid(index: u64) -> Result<UnicodeBlockId, WriterError> {
    UnicodeBlockId::new(false, index)
        .map_err(|error| WriterError::InvalidStructure(error.to_string()))
}

fn internal_bid(index: u64) -> Result<UnicodeBlockId, WriterError> {
    UnicodeBlockId::new(true, index)
        .map_err(|error| WriterError::InvalidStructure(error.to_string()))
}

fn page_trailer(page_type: PageType, offset: u64, page_id: UnicodePageId) -> UnicodePageTrailer {
    let signature = page_type.signature(offset, u64::from(page_id));
    UnicodePageTrailer::new(page_type, signature, page_id, 0)
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

fn reserved_map_page_count(amap_index: u64) -> u64 {
    let mut pages = 1;
    if amap_index % 8 == 0 {
        pages += 1;
        if amap_index >= FMAP_FIRST_AMAP && (amap_index - FMAP_FIRST_AMAP) % FMAP_AMAP_COUNT == 0 {
            pages += 1;
        }
        if amap_index >= FPMAP_FIRST_AMAP && (amap_index - FPMAP_FIRST_AMAP) % FPMAP_AMAP_COUNT == 0
        {
            pages += 1;
        }
    }
    pages
}

fn allocate_extent(cursor: &mut u64, size: u64, alignment: u64) -> Result<u64, WriterError> {
    let mut offset = align_up(*cursor, alignment);
    loop {
        let amap_index = offset.checked_sub(FIRST_AMAP).ok_or_else(|| {
            WriterError::InvalidStructure("allocation before first AMap".to_owned())
        })? / AMAP_DATA_SIZE;
        let region_start = FIRST_AMAP
            .checked_add(amap_index.saturating_mul(AMAP_DATA_SIZE))
            .ok_or(WriterError::ValueTooLarge("allocation region"))?;
        let reserved_end = region_start
            .checked_add(reserved_map_page_count(amap_index).saturating_mul(PAGE_SIZE))
            .ok_or(WriterError::ValueTooLarge("allocation map pages"))?;
        if offset < reserved_end {
            offset = align_up(reserved_end, alignment);
        }
        let end = offset
            .checked_add(size)
            .ok_or(WriterError::ValueTooLarge("allocation extent"))?;
        let region_end = region_start
            .checked_add(AMAP_DATA_SIZE)
            .ok_or(WriterError::ValueTooLarge("allocation region"))?;
        if end <= region_end {
            *cursor = end;
            return Ok(offset);
        }
        offset = region_end;
    }
}

fn allocation_file_eof(allocated_end: u64) -> Result<u64, WriterError> {
    let used = allocated_end
        .checked_sub(FIRST_AMAP)
        .ok_or_else(|| WriterError::InvalidStructure("allocation end underflow".to_owned()))?;
    FIRST_AMAP
        .checked_add(
            used.div_ceil(AMAP_DATA_SIZE)
                .max(1)
                .saturating_mul(AMAP_DATA_SIZE),
        )
        .ok_or(WriterError::ValueTooLarge("file EOF"))
}

fn plan_leaf_pages(entry_count: usize, capacity: usize) -> Result<Vec<Range<usize>>, WriterError> {
    if capacity == 0 {
        return Err(WriterError::InvalidStructure(
            "B-tree leaf capacity must be nonzero".to_owned(),
        ));
    }
    if entry_count == 0 {
        return Ok(Vec::new());
    }
    let page_count = entry_count.div_ceil(capacity);
    let minimum = entry_count / page_count;
    let larger_pages = entry_count % page_count;
    let mut pages = Vec::with_capacity(page_count);
    let mut start = 0;
    for page in 0..page_count {
        let size = minimum + usize::from(page < larger_pages);
        pages.push(start..start + size);
        start += size;
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PstFile, UnicodePstFile,
        messaging::store::EntryId,
        ndb::{
            block_id::BlockId,
            byte_index::ByteIndex,
            header::Header,
            page::{
                AllocationMapPage, BTreePage, BlockBTreeEntry, DensityListPage, PageTrailer,
                RootBTreePage, UnicodeDensityListPage,
            },
            root::Root,
        },
        open_store,
    };
    use std::fs::OpenOptions;

    #[test]
    fn arbitrary_nonempty_classes_are_supported_but_calendar_exception_stays_exact() {
        assert!(supported_message_class(
            "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}"
        ));
        assert!(supported_message_class(
            "ipm.ole.class.{00061055-0000-0000-c000-000000000046}"
        ));
        assert!(supported_message_class(
            "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}.Custom"
        ));
        assert!(supported_message_class(
            "IPM.OLE.CLASS.{00061056-0000-0000-C000-000000000046}"
        ));
        assert!(!supported_message_class(""));
    }

    #[test]
    fn compressed_rtf_has_normative_end_reference() -> Result<(), Box<dyn std::error::Error>> {
        for length in 0..=8 {
            let raw = (0..length)
                .map(|index| b'A' + u8::try_from(index).expect("small test index"))
                .collect::<Vec<_>>();
            assert_rtf_container(&raw)?;
        }
        assert_rtf_container(&[0x80, 0xFF])?;
        assert_rtf_container(&vec![b'X'; 4090])?;
        Ok(())
    }

    fn assert_rtf_container(raw: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let container = rtf_container(raw)?;
        assert_eq!(container.len(), rtf_container_len(raw.len())?);
        assert_eq!(
            u32::from_le_bytes(container[0..4].try_into()?),
            u32::try_from(container.len() - 4)?
        );
        assert_eq!(
            u32::from_le_bytes(container[4..8].try_into()?),
            u32::try_from(raw.len())?
        );
        assert_eq!(
            u32::from_le_bytes(container[8..12].try_into()?),
            0x7546_5A4C
        );
        assert_eq!(
            u32::from_le_bytes(container[12..16].try_into()?),
            crate::crc::compute_crc(0, &container[16..])
        );

        let remainder = raw.len() % 8;
        let end_offset = (207 + raw.len()) % 4096;
        let end_reference = (u16::try_from(end_offset)? << 4).to_be_bytes();
        let final_run_size = remainder + 3;
        let final_run = &container[container.len() - final_run_size..];
        assert_eq!(final_run[0], 1_u8 << remainder);
        assert_eq!(&final_run[1..1 + remainder], &raw[raw.len() - remainder..]);
        assert_eq!(&final_run[1 + remainder..], &end_reference);

        let decoded = compressed_rtf::decompress_rtf(&container)?;
        assert_eq!(
            decoded.encode_utf16().collect::<Vec<_>>(),
            raw.iter().copied().map(u16::from).collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn property_context_heap_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let data = property_context(&[
            (0x3001, PropertyValue::Unicode("Checkpoint".to_owned())),
            (0x3602, PropertyValue::Integer32(7)),
        ])?;
        assert_eq!(data[2], 0xEC);
        assert_eq!(data[3], HeapNodeType::Properties as u8);
        assert_eq!(u16::from_le_bytes([data[0], data[1]]) % 2, 0);
        let expected = "Checkpoint"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(data.windows(expected.len()).any(|bytes| bytes == expected));
        let encoded = unicode_bytes("Checkpoint")?;
        assert_eq!(encoded.len(), "Checkpoint".encode_utf16().count() * 2);
        assert_ne!(encoded.get(encoded.len() - 2..), Some(&[0, 0][..]));
        let empty = property_context(&[(0x0004, PropertyValue::Binary(Vec::new()))])?;
        assert!(
            empty
                .windows(8)
                .any(|record| record == [0x04, 0x00, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00])
        );
        assert_eq!(
            table_variable_bytes(&PropertyValue::Unicode("Checkpoint".to_owned()))?
                .ok_or("missing table string")?
                .len(),
            "Checkpoint".encode_utf16().count() * 2
        );
        Ok(())
    }

    #[test]
    fn allocation_map_marks_only_written_region() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("allocation.pst");
        create_minimal_store(&path, &MinimalStore::default())?;
        let mut file = std::fs::File::open(&path)?;
        file.seek(SeekFrom::Start(FIRST_AMAP))?;
        let page = <UnicodeMapPage<{ PageType::AllocationMap as u8 }> as MapPageReadWrite<
            UnicodePstFile,
            { PageType::AllocationMap as u8 },
        >>::read(&mut file)?;
        let first_free = page.find_free_bits(1).start;
        assert!(first_free > 16);
        let density =
            <UnicodeDensityListPage as DensityListPageReadWrite<UnicodePstFile>>::read(&mut file)?;
        assert!(density.backfill_complete());
        let pst = UnicodePstFile::open(&path)?;
        assert_eq!(density.trailer().block_id(), pst.header().next_page());
        assert_eq!(density.entries().len(), 1);
        assert_eq!(
            u64::from(density.entries()[0].free_slots()),
            SLOTS_PER_AMAP - u64::from(first_free)
        );
        Ok(())
    }

    #[test]
    fn btree_leaf_planning_splits_at_ms_pst_capacity() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(plan_leaf_pages(15, 15)?, vec![0..15]);
        assert_eq!(plan_leaf_pages(16, 15)?, vec![0..8, 8..16]);
        assert_eq!(plan_leaf_pages(31, 15)?, vec![0..11, 11..21, 21..31]);
        assert!(plan_leaf_pages(1, 0).is_err());
        Ok(())
    }

    #[test]
    fn scanpst_required_metadata_is_serialized() -> Result<(), Box<dyn std::error::Error>> {
        let outgoing = outgoing_queue_columns()?;
        assert_eq!(
            outgoing
                .iter()
                .find(|column| column.prop_id() == 0x0039)
                .map(TableColumnDescriptor::prop_type),
            Some(PropertyType::Time)
        );
        assert_eq!(
            outgoing
                .iter()
                .find(|column| column.prop_id() == 0x0E14)
                .map(TableColumnDescriptor::prop_type),
            Some(PropertyType::Integer32)
        );
        assert!(outgoing.iter().all(|column| column.prop_id() != 0x1039));
        assert!(search_index_columns()?.iter().any(|column| {
            column.prop_id() == 0x0E3E && column.prop_type() == PropertyType::Binary
        }));

        let receive = receive_folder_columns()?;
        let mut row = vec![0_u8; 32];
        let mut variables = Vec::new();
        write_table_value(
            &mut row,
            &receive,
            0x001A,
            &PropertyValue::Unicode(String::new()),
            &mut variables,
        )?;
        let class = receive
            .iter()
            .find(|column| column.prop_id() == 0x001A)
            .ok_or("missing receive class")?;
        assert_eq!(
            &row[usize::from(class.offset())..usize::from(class.offset()) + 4],
            &[0, 0, 0, 0]
        );
        assert!(variables.is_empty());

        let minimal = MinimalStore::default();
        let mut spec = FidelityStore::from(&minimal);
        spec.message.raw_properties.extend([
            RawProperty {
                id: 0x0017,
                value: RawPropertyValue::Integer32(1),
            },
            RawProperty {
                id: 0x0070,
                value: RawPropertyValue::Unicode("conversation topic".to_owned()),
            },
            RawProperty {
                id: 0x0071,
                value: RawPropertyValue::Binary(vec![1, 2, 3]),
            },
        ]);
        let message = node(NodeIdType::NormalMessage, MESSAGE_INDEX)?;
        let recipients = table_context(&recipient_columns()?, &[])?;
        let attachments = table_context(&attachment_columns()?, &[])?;
        let record_key = message_record_key(spec.record_key, message);
        let mut properties = message_properties(&spec.message, false, &[], record_key, 0)?;
        let message_size = i32::try_from(
            property_context(&properties)?.len() + recipients.len() + attachments.len(),
        )?;
        set_message_size(&mut properties, message_size)?;
        let contents = contents_columns()?;
        let row = message_table_row(
            message,
            &spec.message,
            spec.record_key,
            record_key,
            message_size,
            &contents,
        )?;
        let contents_ids = contents
            .iter()
            .map(|column| column.prop_id())
            .collect::<BTreeSet<_>>();
        for (id, value) in properties
            .iter()
            .filter(|(id, _)| contents_ids.contains(id))
        {
            assert!(
                row.values
                    .iter()
                    .find_map(|(row_id, row_value)| (*row_id == *id).then_some(row_value))
                    .is_some_and(|row_value| row_value == value),
                "contents row did not copy message property 0x{id:04X}"
            );
        }
        assert!(
            row.values
                .iter()
                .any(|(id, value)| *id == 0x0E17 && matches!(value, PropertyValue::Integer32(0)))
        );
        assert!(row.values.iter().all(|(id, _)| *id != 0x0E03));
        assert!(row.values.iter().any(|(id, _)| *id == 0x0E04));

        let mut no_recipients = spec.clone();
        no_recipients.message.recipients.clear();
        let no_recipient_row = message_table_row(
            message,
            &no_recipients.message,
            no_recipients.record_key,
            record_key,
            message_size,
            &contents,
        )?;
        assert!(
            no_recipient_row
                .values
                .iter()
                .all(|(id, _)| !matches!(*id, 0x0E03 | 0x0E04))
        );
        assert!(row.values.iter().any(|(id, value)| {
            *id == 0x0E33 && matches!(value, PropertyValue::Integer64(0x90))
        }));
        assert!(row.values.iter().any(|(id, value)| {
            *id == 0x0E30 && matches!(value, PropertyValue::Binary(bytes) if bytes == &record_key)
        }));
        let instance_entry_id = [
            1_u32.to_le_bytes().as_slice(),
            spec.record_key.as_slice(),
            1_u32.to_le_bytes().as_slice(),
        ]
        .concat();
        assert_eq!(
            message_instance_entry_id(spec.record_key),
            instance_entry_id
        );
        assert!(row.values.iter().any(|(id, value)| {
            *id == 0x0E34
                && matches!(value, PropertyValue::Binary(bytes) if bytes == &instance_entry_id)
        }));
        Ok(())
    }

    #[test]
    fn new_store_round_trips_through_upstream_reader() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("writer.pst");
        let spec = MinimalStore::default();
        let ipm_folder = node(NodeIdType::NormalFolder, IPM_FOLDER_INDEX)?;
        let search_root = node(NodeIdType::NormalFolder, SEARCH_ROOT_INDEX)?;
        let spam_search = node(NodeIdType::SearchFolder, SPAM_SEARCH_INDEX)?;
        let mail_folder = node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX)?;
        let message_node = node(NodeIdType::NormalMessage, MESSAGE_INDEX)?;
        create_minimal_store(&path, &spec)?;

        let mut header_bytes = std::fs::File::open(&path)?;
        header_bytes.seek(SeekFrom::Start(24))?;
        assert_eq!(
            byteorder::ReadBytesExt::read_u64::<LittleEndian>(&mut header_bytes)?,
            0x0000_0001_0000_0004
        );
        header_bytes.seek(SeekFrom::Start(44))?;
        let mut counters = [0_u32; 32];
        for counter in &mut counters {
            *counter = byteorder::ReadBytesExt::read_u32::<LittleEndian>(&mut header_bytes)?;
        }
        assert_eq!(
            counters[NodeIdType::NormalFolder as usize],
            MAIL_FOLDER_INDEX + 1
        );
        assert_eq!(
            counters[NodeIdType::NormalMessage as usize],
            MESSAGE_INDEX + 1
        );
        assert_eq!(
            counters[NodeIdType::HierarchyTable as usize],
            MAIL_FOLDER_INDEX + 1
        );
        assert_eq!(
            counters[NodeIdType::ContentsTable as usize],
            MAIL_FOLDER_INDEX + 1
        );
        assert_eq!(
            counters[NodeIdType::AssociatedContentsTable as usize],
            MAIL_FOLDER_INDEX + 1
        );

        let pst = UnicodePstFile::open(&path)?;
        assert_eq!(
            pst.header().version(),
            crate::ndb::header::NdbVersion::Unicode
        );
        assert_eq!(pst.header().crypt_method(), NdbCryptMethod::Permute);
        assert_eq!(pst.header().unique_value(), 2);
        assert_eq!(u64::from(pst.header().next_page()), 0x107);
        assert_eq!(pst.header().next_block().index(), 28);
        let root = pst.header().root();
        let mut reader = pst.reader().lock().map_err(|_| "reader lock failed")?;
        let nbt = crate::ndb::page::UnicodeNodeBTree::read(&mut *reader, *root.node_btree())?;
        let bbt = crate::ndb::page::UnicodeBlockBTree::read(&mut *reader, *root.block_btree())?;
        let RootBTreePage::Intermediate(nbt_root, _) = &nbt else {
            return Err("expected an intermediate NBT root".into());
        };
        let RootBTreePage::Intermediate(bbt_root, _) = &bbt else {
            return Err("expected an intermediate BBT root".into());
        };
        assert_eq!(nbt_root.entries().len(), 3);
        assert_eq!(bbt_root.entries().len(), 2);
        let expected_ref_counts = [
            2, 2, 2, 2, 6, 2, 2, 2, 5, 2, 2, 2, 7, 2, 2, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2,
        ];
        let mut bbt_cache = Default::default();
        for (index, expected) in expected_ref_counts.into_iter().enumerate() {
            let index = u64::try_from(index)? + 1;
            let id = if index < 27 {
                leaf_bid(index)?
            } else {
                internal_bid(index)?
            };
            let entry = bbt.find_entry(&mut *reader, id.search_key(), &mut bbt_cache)?;
            assert_eq!(entry.ref_count(), expected);
        }
        let mut nbt_cache = Default::default();
        let root_entry = nbt.find_entry(
            &mut *reader,
            u64::from(u32::from(NID_ROOT_FOLDER)),
            &mut nbt_cache,
        )?;
        assert_eq!(root_entry.parent(), Some(root_entry.node()));
        for id in [ipm_folder, mail_folder, message_node] {
            let entry = nbt.find_entry(&mut *reader, u64::from(u32::from(id)), &mut nbt_cache)?;
            assert_ne!(entry.parent(), Some(entry.node()));
        }
        for raw in [
            u32::from(NID_SEARCH_MANAGEMENT_QUEUE),
            u32::from(NID_SEARCH_ACTIVITY_LIST),
            NID_HIERARCHY_TABLE_TEMPLATE,
            NID_CONTENTS_TABLE_TEMPLATE,
            NID_ASSOC_CONTENTS_TABLE_TEMPLATE,
            NID_SEARCH_CONTENTS_TABLE_TEMPLATE,
            NID_RECIPIENT_TABLE_TEMPLATE,
            NID_ATTACHMENT_TABLE_TEMPLATE,
            NID_RECEIVE_FOLDER_TABLE,
            NID_OUTGOING_QUEUE_TABLE,
            NID_CONTENTS_INDEX_TEMPLATE,
            NID_SEARCH_INDEX_TEMPLATE,
            NID_ATTACHMENT_INDEX_TEMPLATE,
            NID_HIERARCHY_MAP,
            NID_SEARCH_FOLDER_TEMPLATE,
            u32::from(node(NodeIdType::SearchUpdateQueue, SPAM_SEARCH_INDEX)?),
            u32::from(node(NodeIdType::SearchCriteria, SPAM_SEARCH_INDEX)?),
            u32::from(node(NodeIdType::SearchContentsTable, SPAM_SEARCH_INDEX)?),
        ] {
            nbt.find_entry(&mut *reader, u64::from(raw), &mut nbt_cache)?;
        }
        drop(reader);

        let store = open_store(&path)?;
        assert_eq!(store.properties().display_name()?, spec.store_name);
        let hierarchy = store.root_hierarchy_table()?;
        assert_eq!(hierarchy.context().columns().len(), 13);
        assert_eq!(hierarchy.rows_matrix().count(), 3);
        for id in [ipm_folder, search_root, spam_search] {
            hierarchy.find_row(crate::ltp::table_context::TableRowId::new(u32::from(id)))?;
        }
        assert!(
            hierarchy
                .find_row(crate::ltp::table_context::TableRowId::new(u32::from(
                    ipm_folder
                )))?
                .columns(hierarchy.context())?
                .iter()
                .filter(|value| value.is_some())
                .count()
                == 7
        );

        let ipm_entry = store.properties().make_entry_id(ipm_folder)?;
        let ipm = store.open_folder(&ipm_entry)?;
        assert_eq!(
            ipm.associated_table()
                .ok_or("missing IPM associated contents")?
                .rows_matrix()
                .count(),
            0
        );
        let ipm_hierarchy = ipm.hierarchy_table().ok_or("missing IPM hierarchy")?;
        assert_eq!(ipm_hierarchy.rows_matrix().count(), 2);
        ipm_hierarchy.find_row(crate::ltp::table_context::TableRowId::new(u32::from(
            mail_folder,
        )))?;

        let mail_entry = store.properties().make_entry_id(mail_folder)?;
        let mail = store.open_folder(&mail_entry)?;
        let contents = mail.contents_table().ok_or("missing mail contents")?;
        assert_eq!(contents.rows_matrix().count(), 1);
        let row = contents.find_row(crate::ltp::table_context::TableRowId::new(u32::from(
            message_node,
        )))?;
        let values = row.columns(contents.context())?;
        let size_column = contents
            .context()
            .columns()
            .iter()
            .position(|column| column.prop_id() == 0x0E08)
            .ok_or("missing message-size column")?;
        let row_size = match values.get(size_column) {
            Some(Some(crate::ltp::table_context::TableRowColumnValue::Small(
                crate::ltp::prop_context::PropertyValue::Integer32(value),
            ))) => *value,
            _ => return Err("invalid message-size row".into()),
        };
        let entry_id = EntryId::new(
            crate::messaging::store::StoreRecordKey::new(spec.record_key),
            message_node,
        );
        let message = store.open_message(&entry_id, None)?;
        assert_eq!(message.properties().message_class()?, "IPM.Note");
        assert_eq!(message.properties().message_size()?, row_size);
        let recipients = message
            .recipient_table()
            .ok_or("missing required recipient table")?;
        assert_eq!(recipients.context().columns().len(), 14);
        assert_eq!(recipients.rows_matrix().count(), 1);
        Ok(())
    }

    #[test]
    fn multi_message_store_indexes_each_top_level_message() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("multiple.pst");
        let base = FidelityStore::from(&MinimalStore {
            store_name: "PSTForge multi-message".to_owned(),
            folder_name: "Inbox".to_owned(),
            subject: "first".to_owned(),
            body: "first body".to_owned(),
            sender_name: "Sender".to_owned(),
            sender_email: "sender@example.com".to_owned(),
            recipient: "recipient@example.com".to_owned(),
            record_key: *b"PSTForgeMultiMsg",
        });
        let mut second = base.message.clone();
        second.subject = "second".to_owned();
        second.body_text = Some("second body".to_owned());
        let spec = MailStoreSpec {
            store_name: base.store_name.clone(),
            record_key: base.record_key,
            folders: vec![MailFolderSpec {
                path: vec![base.folder_name.clone()],
                location: MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: vec![base.message.clone(), second],
                associated_messages: Vec::new(),
            }],
        };
        create_mail_store(&path, &spec)?;

        let store = open_store(&path)?;
        let folder = store.open_folder(
            &store
                .properties()
                .make_entry_id(node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX)?)?,
        )?;
        assert_eq!(
            folder
                .contents_table()
                .ok_or("missing contents table")?
                .rows_matrix()
                .count(),
            2
        );
        let second = store.open_message(
            &EntryId::new(
                crate::messaging::store::StoreRecordKey::new(spec.record_key),
                node(NodeIdType::NormalMessage, MESSAGE_INDEX + 1)?,
            ),
            None,
        )?;
        assert!(matches!(
            second.properties().get(0x0037),
            Some(crate::ltp::prop_context::PropertyValue::Unicode(value))
                if value.to_string() == "second"
        ));
        Ok(())
    }

    #[test]
    fn root_folders_and_associated_messages_keep_their_source_placement()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("root-associated.pst");
        let base = FidelityStore::default();
        let mut sniff = base.message.clone();
        sniff.message_class = "IPM.Microsoft.SniffData".to_owned();
        sniff.subject = "structural root item".to_owned();
        sniff.message_flags |= MSGFLAG_ASSOCIATED;
        sniff.sender_name.clear();
        sniff.sender_email.clear();
        let mut configuration = sniff.clone();
        configuration.message_class = "IPM.Configuration.PSTForge".to_owned();
        configuration.subject = "subject fallback must not replace display name".to_owned();
        configuration.raw_properties = vec![RawProperty {
            id: 0x3001,
            value: RawPropertyValue::Unicode("hidden associated item".to_owned()),
        }];
        let spec = MailStoreSpec {
            store_name: "PSTForge root and associated placement".to_owned(),
            record_key: *b"PSTForgeAssoc001",
            folders: vec![
                MailFolderSpec {
                    path: vec!["Freebusy Data".to_owned()],
                    location: MailFolderLocation::StoreRoot,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![sniff],
                    associated_messages: Vec::new(),
                },
                MailFolderSpec {
                    path: vec!["IPM_COMMON_VIEWS".to_owned()],
                    location: MailFolderLocation::StoreRoot,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Note".to_owned(),
                    messages: Vec::new(),
                    associated_messages: vec![configuration],
                },
            ],
        };
        create_mail_store(&path, &spec)?;

        let store = open_store(&path)?;
        let root = store.open_folder(&store.properties().make_entry_id(NID_ROOT_FOLDER)?)?;
        let hierarchy = root.hierarchy_table().ok_or("missing root hierarchy")?;
        assert_eq!(hierarchy.rows_matrix().count(), 5);
        let freebusy_node = node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX)?;
        let views_node = node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX + 1)?;
        for folder in [freebusy_node, views_node] {
            hierarchy.find_row(crate::ltp::table_context::TableRowId::new(u32::from(
                folder,
            )))?;
        }
        let freebusy = store.open_folder(&store.properties().make_entry_id(freebusy_node)?)?;
        assert_eq!(
            freebusy
                .contents_table()
                .ok_or("missing Freebusy contents")?
                .rows_matrix()
                .count(),
            1
        );
        let normal_message = store.open_message(
            &store
                .properties()
                .make_entry_id(node(NodeIdType::NormalMessage, MESSAGE_INDEX)?)?,
            Some(&[0x0E07]),
        )?;
        assert!(matches!(
            normal_message.properties().get(0x0E07),
            Some(crate::ltp::prop_context::PropertyValue::Integer32(value))
                if *value & MSGFLAG_ASSOCIATED == 0
        ));
        assert_eq!(
            freebusy
                .associated_table()
                .ok_or("missing Freebusy associated contents")?
                .rows_matrix()
                .count(),
            0
        );
        let views = store.open_folder(&store.properties().make_entry_id(views_node)?)?;
        assert_eq!(
            views
                .contents_table()
                .ok_or("missing views contents")?
                .rows_matrix()
                .count(),
            0
        );
        let associated_table = views
            .associated_table()
            .ok_or("missing views associated contents")?;
        assert_eq!(associated_table.rows_matrix().count(), 1);
        let associated_row =
            associated_table.find_row(crate::ltp::table_context::TableRowId::new(u32::from(
                node(NodeIdType::AssociatedMessage, MESSAGE_INDEX + 1)?,
            )))?;
        let associated_values = associated_row.columns(associated_table.context())?;
        let display_name_column = associated_table
            .context()
            .columns()
            .iter()
            .position(|column| column.prop_id() == 0x3001)
            .ok_or("missing associated display-name column")?;
        let display_name = associated_values[display_name_column]
            .as_ref()
            .ok_or("missing associated display-name value")?;
        assert!(matches!(
            associated_table.read_column(
                display_name,
                associated_table.context().columns()[display_name_column].prop_type(),
            )?,
            crate::ltp::prop_context::PropertyValue::Unicode(value)
                if value.to_string() == "hidden associated item"
        ));
        let associated_message = store.open_message(
            &store
                .properties()
                .make_entry_id(node(NodeIdType::AssociatedMessage, MESSAGE_INDEX + 1)?)?,
            Some(&[0x0E07]),
        )?;
        assert!(matches!(
            associated_message.properties().get(0x0E07),
            Some(crate::ltp::prop_context::PropertyValue::Integer32(value))
                if *value & MSGFLAG_ASSOCIATED != 0
        ));

        let associated_only = MailStoreSpec {
            store_name: "PSTForge associated-only placement".to_owned(),
            record_key: *b"PSTForgeAssocOnl",
            folders: vec![spec.folders[1].clone()],
        };
        let associated_only_path = directory.path().join("associated-only.pst");
        create_mail_store(&associated_only_path, &associated_only)?;
        let associated_store = open_store(&associated_only_path)?;
        associated_store.open_message(
            &associated_store
                .properties()
                .make_entry_id(node(NodeIdType::AssociatedMessage, MESSAGE_INDEX)?)?,
            Some(&[]),
        )?;
        Ok(())
    }

    #[test]
    fn multi_message_validation_uses_the_store_wide_named_property_map()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("multiple-named-sets.pst");
        let base = FidelityStore::default();
        let named = |guid_first, subject: &str| {
            let mut message = base.message.clone();
            message.subject = subject.to_owned();
            message.named_properties = vec![NamedProperty {
                set: NamedPropertySet::Guid([
                    guid_first, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x00, 0x46,
                ]),
                name: NamedPropertyName::Numeric(0x8101),
                value: RawPropertyValue::Integer32(0),
            }];
            message
        };
        let spec = MailStoreSpec {
            store_name: "PSTForge named map validation".to_owned(),
            record_key: *b"PSTForgeNamedMap",
            folders: vec![
                MailFolderSpec {
                    path: vec!["Notes".to_owned()],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![named(0x0E, "lexically first folder")],
                    associated_messages: Vec::new(),
                },
                MailFolderSpec {
                    path: vec!["Tasks".to_owned()],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Task".to_owned(),
                    messages: vec![named(0x03, "globally first named property")],
                    associated_messages: Vec::new(),
                },
            ],
        };
        create_mail_store(&path, &spec)?;
        assert!(path.is_file());
        Ok(())
    }

    #[test]
    fn contact_message_round_trips_in_contact_folder() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("contact.pst");
        let mut message = FidelityStore::default().message;
        message.message_class = "IPM.Contact".to_owned();
        message.subject = "Ada Lovelace".to_owned();
        message.sender_name.clear();
        message.sender_email.clear();
        message.recipients.clear();
        message.body_text = None;
        message.native_body = None;
        message.raw_properties = vec![
            RawProperty {
                id: 0x3001,
                value: RawPropertyValue::Unicode("Ada Lovelace".to_owned()),
            },
            RawProperty {
                id: 0x3A06,
                value: RawPropertyValue::Unicode("Ada".to_owned()),
            },
            RawProperty {
                id: 0x3A11,
                value: RawPropertyValue::Unicode("Lovelace".to_owned()),
            },
        ];
        message.named_properties = vec![NamedProperty {
            set: NamedPropertySet::Guid([
                0x04, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x46,
            ]),
            name: NamedPropertyName::Numeric(0x8083),
            value: RawPropertyValue::Unicode("ada@example.com".to_owned()),
        }];
        let spec = MailStoreSpec {
            store_name: "PSTForge contact".to_owned(),
            record_key: *b"PSTForgeContact1",
            folders: vec![MailFolderSpec {
                path: vec!["Contacts".to_owned()],
                location: MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Contact".to_owned(),
                messages: vec![message],
                associated_messages: Vec::new(),
            }],
        };
        create_mail_store(&path, &spec)?;

        let store = open_store(&path)?;
        let folder = store.open_folder(
            &store
                .properties()
                .make_entry_id(node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX)?)?,
        )?;
        assert!(matches!(
            folder.properties().get(0x3613),
            Some(crate::ltp::prop_context::PropertyValue::Unicode(value))
                if value.to_string() == "IPF.Contact"
        ));
        let contact = store.open_message(
            &EntryId::new(
                crate::messaging::store::StoreRecordKey::new(spec.record_key),
                node(NodeIdType::NormalMessage, MESSAGE_INDEX)?,
            ),
            None,
        )?;
        assert_eq!(contact.properties().message_class()?, "IPM.Contact");
        assert!(matches!(
            contact.properties().get(0x3A06),
            Some(crate::ltp::prop_context::PropertyValue::Unicode(value))
                if value.to_string() == "Ada"
        ));
        Ok(())
    }

    #[test]
    fn appointment_message_round_trips_in_calendar_folder() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("appointment.pst");
        let mut message = FidelityStore::default().message;
        message.message_class = "IPM.Appointment".to_owned();
        message.subject = "Appointment fidelity checkpoint".to_owned();
        message.sender_name.clear();
        message.sender_email.clear();
        message.recipients.clear();
        message.named_properties = vec![
            NamedProperty {
                set: NamedPropertySet::Guid([
                    0x02, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x46,
                ]),
                name: NamedPropertyName::Numeric(0x820D),
                value: RawPropertyValue::Time(133_814_268_000_000_000),
            },
            NamedProperty {
                set: NamedPropertySet::Guid([
                    0x02, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x46,
                ]),
                name: NamedPropertyName::Numeric(0x820E),
                value: RawPropertyValue::Time(133_814_304_000_000_000),
            },
        ];
        let spec = MailStoreSpec {
            store_name: "PSTForge appointment".to_owned(),
            record_key: *b"PSTForgeAppt0001",
            folders: vec![MailFolderSpec {
                path: vec!["Calendar".to_owned()],
                location: MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Appointment".to_owned(),
                messages: vec![message],
                associated_messages: Vec::new(),
            }],
        };
        create_mail_store(&path, &spec)?;

        let store = open_store(&path)?;
        let folder = store.open_folder(
            &store
                .properties()
                .make_entry_id(node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX)?)?,
        )?;
        assert!(matches!(
            folder.properties().get(0x3613),
            Some(crate::ltp::prop_context::PropertyValue::Unicode(value))
                if value.to_string() == "IPF.Appointment"
        ));
        let appointment = store.open_message(
            &EntryId::new(
                crate::messaging::store::StoreRecordKey::new(spec.record_key),
                node(NodeIdType::NormalMessage, MESSAGE_INDEX)?,
            ),
            None,
        )?;
        assert_eq!(appointment.properties().message_class()?, "IPM.Appointment");
        Ok(())
    }

    #[test]
    fn private_message_tables_do_not_retain_template_refcounts()
    -> Result<(), Box<dyn std::error::Error>> {
        let blocks = built_message_block_specs(BuiltTopMessage {
            property_block: leaf_bid(100)?,
            recipient_block: leaf_bid(101)?,
            attachment_block: leaf_bid(102)?,
            subnode_block: internal_bid(103)?,
            shared_table_blocks: false,
            message: MessageBlocks {
                property_context: Vec::new(),
                recipient_table: Vec::new(),
                attachment_table: Vec::new(),
                subnodes: Vec::new(),
                dynamic_blocks: Vec::new(),
                record_key: [0; 16],
                message_size: 0,
                streamed_logical_size: 0,
            },
        });
        assert_eq!(blocks[1].ref_count, 2);
        assert_eq!(blocks[2].ref_count, 2);
        Ok(())
    }

    #[test]
    fn subnode_tree_rejects_a_second_intermediate_level_before_writing_blocks()
    -> Result<(), Box<dyn std::error::Error>> {
        let entry = UnicodeLeafSubNodeTreeEntry::new(
            node(NodeIdType::ListsTablesProperties, 1)?,
            leaf_bid(1)?,
            None,
        );
        let entries = vec![entry; MAX_SUBNODE_TREE_ENTRIES + 1];
        let mut next_block_index = 2;
        let mut blocks = Vec::new();

        assert!(matches!(
            append_subnode_tree(entries, &mut next_block_index, &mut blocks),
            Err(WriterError::ValueTooLarge("subnode tree entry count"))
        ));
        assert!(blocks.is_empty());
        assert_eq!(next_block_index, 2);
        Ok(())
    }

    #[test]
    fn mail_store_preflight_contains_hierarchy_and_huge_attachment_shapes() {
        let base = FidelityStore::default();
        let many_folders = MailStoreSpec {
            store_name: "PSTForge hierarchy limit".to_owned(),
            record_key: base.record_key,
            folders: (0..1_000)
                .map(|index| MailFolderSpec {
                    path: vec![format!("Folder {index:04}")],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![base.message.clone()],
                    associated_messages: Vec::new(),
                })
                .collect(),
        };
        assert!(matches!(
            validate_mail_store_input(&many_folders),
            Err(WriterError::InputRejected(_))
        ));

        let mut huge_message = base.message;
        huge_message.attachments = vec![AttachmentSpec {
            filename: "huge.bin".to_owned(),
            mime_type: Some("application/octet-stream".to_owned()),
            content_id: None,
            content_location: None,
            rendering_position: None,
            flags: 0,
            raw_properties: Vec::new(),
            content: AttachmentContent::Spooled(FileBlobSpec {
                path: PathBuf::from("/dev/null"),
                offset: 0,
                byte_len: i32::MAX as u64,
                sha256: [0; 32],
            }),
        }];
        let huge_attachment = MailStoreSpec {
            store_name: "PSTForge attachment limit".to_owned(),
            record_key: *b"PSTForgeHugeBlob",
            folders: vec![MailFolderSpec {
                path: vec!["Inbox".to_owned()],
                location: MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: vec![huge_message],
                associated_messages: Vec::new(),
            }],
        };
        assert!(matches!(
            validate_mail_store_input(&huge_attachment),
            Err(WriterError::InputRejected(_))
        ));
    }

    #[test]
    fn spooled_attachment_streams_across_data_tree_groups() -> Result<(), Box<dyn std::error::Error>>
    {
        use crate::messaging::{
            attachment::{Attachment, AttachmentData, UnicodeAttachment},
            message::UnicodeMessage,
            store::UnicodeStore,
        };
        use std::rc::Rc;

        let directory = tempfile::tempdir()?;
        let source = directory.path().join("payload.bin");
        let payload_len = MAX_DATA_BLOCK_PAYLOAD
            .checked_mul(MAX_DATA_TREE_ENTRIES)
            .and_then(|length| length.checked_add(137))
            .ok_or("test payload length overflow")?;
        let payload = (0..payload_len)
            .map(|index| u8::try_from(index % 251).expect("bounded byte"))
            .collect::<Vec<_>>();
        std::fs::write(&source, &payload)?;
        let sha256: [u8; 32] = Sha256::digest(&payload).into();

        let mut spec = FidelityStore::default();
        spec.message.attachments.truncate(1);
        spec.message.attachments[0].content = AttachmentContent::Spooled(FileBlobSpec {
            path: source,
            offset: 0,
            byte_len: u64::try_from(payload.len())?,
            sha256,
        });
        let destination = directory.path().join("spooled.pst");
        create_fidelity_store(&destination, &spec)?;

        let pst = Rc::new(UnicodePstFile::open(&destination)?);
        let store = UnicodeStore::read(pst)?;
        let message = UnicodeMessage::read(
            store,
            &EntryId::new(
                crate::messaging::store::StoreRecordKey::new(spec.record_key),
                node(NodeIdType::NormalMessage, MESSAGE_INDEX)?,
            ),
            None,
        )?;
        let attachment =
            UnicodeAttachment::read(message, node(NodeIdType::Attachment, 0x2_0000)?, None)?;
        assert!(matches!(
            attachment.data(),
            Some(AttachmentData::Binary(actual)) if actual.buffer() == payload
        ));
        Ok(())
    }

    #[test]
    fn spooled_attachment_rejects_stale_identity_without_publication()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("payload.bin");
        let payload = vec![0xA5; MAX_DATA_BLOCK_PAYLOAD + 1];
        std::fs::write(&source, &payload)?;
        let mut spec = FidelityStore::default();
        spec.message.attachments.truncate(1);
        spec.message.attachments[0].content = AttachmentContent::Spooled(FileBlobSpec {
            path: source,
            offset: 0,
            byte_len: u64::try_from(payload.len())?,
            sha256: [0_u8; 32],
        });

        let bad_hash = directory.path().join("bad-hash.pst");
        assert!(matches!(
            create_fidelity_store(&bad_hash, &spec),
            Err(WriterError::InvalidStructure(message)) if message.contains("hash mismatch")
        ));
        assert!(!bad_hash.exists());

        let AttachmentContent::Spooled(blob) = &mut spec.message.attachments[0].content else {
            return Err("expected spooled attachment".into());
        };
        blob.sha256 = Sha256::digest(&payload).into();
        blob.byte_len = blob
            .byte_len
            .checked_add(1)
            .ok_or("test payload length overflow")?;
        let bad_length = directory.path().join("bad-length.pst");
        assert!(matches!(
            create_fidelity_store(&bad_length, &spec),
            Err(WriterError::InvalidStructure(message)) if message.contains("identity mismatch")
        ));
        assert!(!bad_length.exists());
        Ok(())
    }

    #[test]
    fn spooled_message_properties_round_trip_top_level_and_embedded()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let write_utf16_blob = |name: &str,
                                text: &str,
                                repeats: usize|
         -> Result<FileBlobSpec, Box<dyn std::error::Error>> {
            let path = directory.path().join(name);
            let mut file = std::fs::File::create(&path)?;
            let encoded = text
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>();
            let mut hasher = Sha256::new();
            let mut byte_len = 0_u64;
            for _ in 0..repeats {
                file.write_all(&encoded)?;
                hasher.update(&encoded);
                byte_len = byte_len
                    .checked_add(u64::try_from(encoded.len())?)
                    .ok_or("streamed property test size overflow")?;
            }
            file.sync_all()?;
            Ok(FileBlobSpec {
                path,
                offset: 0,
                byte_len,
                sha256: hasher.finalize().into(),
            })
        };

        let mut spec = FidelityStore::default();
        spec.message.body_text = None;
        spec.message.native_body = Some(NativeBody::PlainText);
        spec.message.spooled_properties.push(SpooledPropertySpec {
            id: 0x1000,
            property_type: u16::from(PropertyType::Unicode),
            blob: write_utf16_blob("top-body.bin", "Top streamed body. ", 120_000)?,
        });
        let AttachmentContent::Embedded(embedded) = &mut spec.message.attachments[1].content else {
            return Err("expected embedded message".into());
        };
        embedded.body_text = None;
        embedded.native_body = Some(NativeBody::PlainText);
        embedded.spooled_properties.push(SpooledPropertySpec {
            id: 0x1000,
            property_type: u16::from(PropertyType::Unicode),
            blob: write_utf16_blob("embedded-body.bin", "Embedded streamed body. ", 2_000)?,
        });

        create_fidelity_store(directory.path().join("streamed-properties.pst"), &spec)?;
        Ok(())
    }

    #[test]
    fn completed_store_hashes_spooled_attachments_after_first_message()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("later.bin");
        let payload = vec![0x3C; MAX_DATA_BLOCK_PAYLOAD + 17];
        std::fs::write(&source, &payload)?;

        let base = FidelityStore::default();
        let mut first = base.message.clone();
        first.attachments.clear();
        let mut second = first.clone();
        second.subject = "second with spool".to_owned();
        second.attachments.push(AttachmentSpec {
            filename: "later.bin".to_owned(),
            mime_type: Some("application/octet-stream".to_owned()),
            content_id: None,
            content_location: None,
            rendering_position: None,
            flags: 0,
            raw_properties: Vec::new(),
            content: AttachmentContent::Spooled(FileBlobSpec {
                path: source,
                offset: 0,
                byte_len: u64::try_from(payload.len())?,
                sha256: Sha256::digest(&payload).into(),
            }),
        });
        let mut spec = MailStoreSpec {
            store_name: base.store_name,
            record_key: base.record_key,
            folders: vec![MailFolderSpec {
                path: vec![base.folder_name],
                location: MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages: vec![first, second],
                associated_messages: Vec::new(),
            }],
        };
        let destination = directory.path().join("later.pst");
        create_mail_store(&destination, &spec)?;

        let AttachmentContent::Spooled(blob) =
            &mut spec.folders[0].messages[1].attachments[0].content
        else {
            return Err("expected spooled attachment".into());
        };
        blob.sha256 = [0_u8; 32];
        let plans = plan_folders("unused", &[], Some(&spec.folders))?;
        assert!(matches!(
            validate_spooled_attachment_identities(&destination, spec.record_key, &plans),
            Err(WriterError::InvalidStructure(message)) if message.contains("identity mismatch")
        ));
        Ok(())
    }

    #[test]
    fn spooled_attachment_validates_above_reader_safety_thresholds()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("large.bin");
        let mut source_file = std::fs::File::create(&source)?;
        let chunk = vec![0x6D; 1024 * 1024];
        let mut hasher = Sha256::new();
        for _ in 0..129 {
            source_file.write_all(&chunk)?;
            hasher.update(&chunk);
        }
        source_file.sync_all()?;
        drop(source_file);

        let mut spec = FidelityStore::default();
        spec.message.attachments.truncate(1);
        spec.message.attachments[0].content = AttachmentContent::Spooled(FileBlobSpec {
            path: source,
            offset: 0,
            byte_len: 129 * 1024 * 1024,
            sha256: hasher.finalize().into(),
        });
        create_fidelity_store(directory.path().join("large.pst"), &spec)?;
        Ok(())
    }

    #[test]
    #[ignore = "writes and independently validates a PST larger than the first FPMap boundary"]
    fn spooled_attachment_validates_across_first_fpmap_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        const BYTE_LEN: u64 = 2_081_000_000;
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("sparse-zeroes.bin");
        let source_file = std::fs::File::create(&source)?;
        source_file.set_len(BYTE_LEN)?;
        source_file.sync_all()?;
        drop(source_file);

        let zeroes = vec![0_u8; 1024 * 1024];
        let mut remaining = BYTE_LEN;
        let mut hasher = Sha256::new();
        while remaining != 0 {
            let length_u64 = remaining.min(u64::try_from(zeroes.len())?);
            let length = usize::try_from(length_u64)?;
            hasher.update(&zeroes[..length]);
            remaining = remaining
                .checked_sub(length_u64)
                .ok_or("hash length underflow")?;
        }

        let mut spec = FidelityStore::default();
        spec.message.attachments.truncate(1);
        spec.message.attachments[0].content = AttachmentContent::Spooled(FileBlobSpec {
            path: source,
            offset: 0,
            byte_len: BYTE_LEN,
            sha256: hasher.finalize().into(),
        });
        let output = directory.path().join("crosses-first-fpmap.pst");
        create_fidelity_store(&output, &spec)?;
        assert!(output.metadata()?.len() > crate::FPMAP_FIRST_OFFSET);
        let pst = UnicodePstFile::open(&output)?;
        assert_eq!(
            output.metadata()?.len(),
            pst.header().root().file_eof_index().index()
        );
        Ok(())
    }

    #[test]
    fn first_fpmap_region_reserves_the_fpmap_before_message_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let region = FIRST_AMAP
            .checked_add(FPMAP_FIRST_AMAP * AMAP_DATA_SIZE)
            .ok_or("FPMap region overflow")?;
        let mut cursor = region;
        let allocated = allocate_extent(&mut cursor, SLOT_SIZE, SLOT_SIZE)?;
        assert_eq!(crate::FPMAP_FIRST_OFFSET, region + 2 * PAGE_SIZE);
        assert_eq!(allocated, region + 3 * PAGE_SIZE);
        Ok(())
    }

    #[test]
    fn contents_table_scales_past_one_heap_page() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("many-messages.pst");
        let base = FidelityStore::from(&MinimalStore {
            store_name: "PSTForge scalable contents".to_owned(),
            folder_name: "Inbox".to_owned(),
            subject: "message 00".to_owned(),
            body: "body".to_owned(),
            sender_name: "Sender".to_owned(),
            sender_email: "sender@example.com".to_owned(),
            recipient: "recipient@example.com".to_owned(),
            record_key: *b"PSTForgeScale001",
        });
        let messages = (0..1100)
            .map(|index| MessageSpec {
                subject: format!("message {index:02}"),
                ..base.message.clone()
            })
            .collect::<Vec<_>>();
        let spec = MailStoreSpec {
            store_name: base.store_name.clone(),
            record_key: base.record_key,
            folders: vec![MailFolderSpec {
                path: vec![base.folder_name.clone()],
                location: MailFolderLocation::IpmSubtree,
                role: MailFolderRole::Ordinary,
                container_class: "IPF.Note".to_owned(),
                messages,
                associated_messages: Vec::new(),
            }],
        };
        create_mail_store(&path, &spec)?;

        let pst = UnicodePstFile::open(&path)?;
        assert!(pst.header().root().file_eof_index().index() > INITIAL_FILE_EOF);
        let mut raw = std::fs::File::open(&path)?;
        raw.seek(SeekFrom::Start(FIRST_AMAP + AMAP_DATA_SIZE))?;
        <UnicodeMapPage<{ PageType::AllocationMap as u8 }> as MapPageReadWrite<
            UnicodePstFile,
            { PageType::AllocationMap as u8 },
        >>::read(&mut raw)?;

        let store = open_store(&path)?;
        let folder = store.open_folder(
            &store
                .properties()
                .make_entry_id(node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX)?)?,
        )?;
        let contents = folder.contents_table().ok_or("missing contents table")?;
        assert_eq!(contents.rows_matrix().count(), 1100);
        let rows_per_block =
            MAX_DATA_BLOCK_PAYLOAD / usize::from(contents.context().end_existence_bitmap());
        for index in [rows_per_block - 1, rows_per_block] {
            let expected = crate::ltp::table_context::TableRowId::new(u32::from(node(
                NodeIdType::NormalMessage,
                MESSAGE_INDEX + u32::try_from(index)?,
            )?));
            assert_eq!(
                contents
                    .rows_matrix()
                    .nth(index)
                    .ok_or("missing boundary matrix row")?
                    .id(),
                expected
            );
            assert_eq!(contents.find_row(expected)?.id(), expected);
        }
        Ok(())
    }

    #[test]
    fn external_table_fills_every_non_final_heap_page() -> Result<(), Box<dyn std::error::Error>> {
        let columns = contents_columns()?;
        let rows = (0..8000)
            .map(|index| {
                Ok(TableRowSpec {
                    id: node(NodeIdType::NormalMessage, MESSAGE_INDEX + index)?,
                    values: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, WriterError>>()?;
        let mut next_block = 100;
        let external = table_context_external(&columns, &rows, &mut next_block)?;
        let root = external
            .blocks
            .iter()
            .find(|block| block.id == external.data_block)
            .ok_or("missing external table root")?;
        let BlockPayload::DataTree {
            total_size,
            entries,
            ..
        } = &root.payload
        else {
            return Err("external table root is not an XBLOCK".into());
        };
        assert!(entries.len() > 1);
        let child_sizes = entries
            .iter()
            .map(|entry| {
                let id = UnicodeBlockId::from(*entry);
                let block = external
                    .blocks
                    .iter()
                    .find(|block| block.id == id)
                    .ok_or("missing XBLOCK child")?;
                let BlockPayload::Data(data) = &block.payload else {
                    return Err("XBLOCK child is not a data block");
                };
                Ok(data.len())
            })
            .collect::<Result<Vec<_>, &str>>()?;
        assert!(
            child_sizes[..child_sizes.len() - 1]
                .iter()
                .all(|size| *size == MAX_DATA_BLOCK_PAYLOAD)
        );
        assert_eq!(
            usize::try_from(*total_size)?,
            child_sizes.iter().sum::<usize>()
        );
        for (page_index, entry) in entries[..entries.len() - 1].iter().enumerate() {
            let id = UnicodeBlockId::from(*entry);
            let block = external
                .blocks
                .iter()
                .find(|block| block.id == id)
                .ok_or("missing XBLOCK child")?;
            let BlockPayload::Data(data) = &block.payload else {
                return Err("XBLOCK child is not a data block".into());
            };
            let map_offset = usize::from(u16::from_le_bytes(data[..2].try_into()?));
            let page_map = data.get(map_offset..).ok_or("heap map exceeds page")?;
            let allocation_count = usize::from(u16::from_le_bytes(page_map[..2].try_into()?));
            assert_eq!(page_map.len(), 4 + (allocation_count + 1) * 2);
            let offsets = page_map[4..]
                .chunks_exact(2)
                .map(|bytes| u16::from_le_bytes(bytes.try_into().expect("two-byte chunk")))
                .collect::<Vec<_>>();
            assert_eq!(
                usize::from(*offsets.last().ok_or("missing endpoint")?),
                map_offset
            );
            assert!(
                offsets
                    .windows(2)
                    .all(|range| usize::from(range[1] - range[0]) <= 3580)
            );
            if page_index == 1 {
                assert!(allocation_count > 1);
                assert_eq!(offsets[..2], [2, 3578]);
            }
        }
        let root_data = external
            .blocks
            .iter()
            .find(|block| block.id == UnicodeBlockId::from(entries[0]))
            .and_then(|block| match &block.payload {
                BlockPayload::Data(data) => Some(data),
                _ => None,
            })
            .ok_or("missing root heap page")?;
        let root_header = HeapNodeHeader::read(&mut io::Cursor::new(root_data))?;
        assert!(
            root_header
                .fill_levels()
                .iter()
                .all(|level| *level == HeapFillLevel::Level15)
        );
        let bitmap_data = external
            .blocks
            .iter()
            .find(|block| block.id == UnicodeBlockId::from(entries[8]))
            .and_then(|block| match &block.payload {
                BlockPayload::Data(data) => Some(data),
                _ => None,
            })
            .ok_or("missing bitmap heap page")?;
        let bitmap_header = HeapNodeBitmapHeader::read(&mut io::Cursor::new(bitmap_data))?;
        let full_bitmap_pages = entries.len() - 1 - 8;
        assert!(
            bitmap_header.fill_levels()[..full_bitmap_pages]
                .iter()
                .all(|level| *level == HeapFillLevel::Level15)
        );
        assert!(
            bitmap_header.fill_levels()[full_bitmap_pages..]
                .iter()
                .all(|level| *level == HeapFillLevel::Empty)
        );
        for tree in external.blocks.iter().filter_map(|block| {
            let BlockPayload::DataTree {
                level: 1, entries, ..
            } = &block.payload
            else {
                return None;
            };
            (entries.len() > 1).then_some(entries)
        }) {
            let child_sizes = tree
                .iter()
                .map(|entry| {
                    let id = UnicodeBlockId::from(*entry);
                    let block = external
                        .blocks
                        .iter()
                        .find(|block| block.id == id)
                        .ok_or("missing XBLOCK child")?;
                    let BlockPayload::Data(data) = &block.payload else {
                        return Err("XBLOCK child is not a data block");
                    };
                    Ok(data.len())
                })
                .collect::<Result<Vec<_>, &str>>()?;
            assert!(
                child_sizes[..child_sizes.len() - 1]
                    .iter()
                    .all(|size| *size == MAX_DATA_BLOCK_PAYLOAD),
                "every non-final XBLOCK child must use the maximum payload"
            );
        }
        Ok(())
    }

    #[test]
    fn mail_store_reproduces_required_nested_folder_paths() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("folders.pst");
        let base = FidelityStore::from(&MinimalStore {
            store_name: "PSTForge hierarchy".to_owned(),
            folder_name: "unused".to_owned(),
            subject: "base".to_owned(),
            body: "body".to_owned(),
            sender_name: "Sender".to_owned(),
            sender_email: "sender@example.com".to_owned(),
            recipient: "recipient@example.com".to_owned(),
            record_key: *b"PSTForgeFolders1",
        });
        let message = |subject: &str| MessageSpec {
            subject: subject.to_owned(),
            ..base.message.clone()
        };
        let spec = MailStoreSpec {
            store_name: base.store_name,
            record_key: base.record_key,
            folders: vec![
                MailFolderSpec {
                    path: vec!["Deleted Items".to_owned()],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::DeletedItems,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![message("deleted")],
                    associated_messages: Vec::new(),
                },
                MailFolderSpec {
                    path: vec!["Deleted items".to_owned()],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![message("user-created deleted items")],
                    associated_messages: Vec::new(),
                },
                MailFolderSpec {
                    path: vec!["Inbox".to_owned(), "Projects".to_owned()],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![message("projects")],
                    associated_messages: Vec::new(),
                },
                MailFolderSpec {
                    path: vec!["Archive".to_owned()],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![message("archive")],
                    associated_messages: Vec::new(),
                },
                MailFolderSpec {
                    path: vec!["Inbox".to_owned(), "Personal".to_owned()],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![message("personal")],
                    associated_messages: Vec::new(),
                },
            ],
        };
        create_mail_store(&path, &spec)?;
        assert_eq!(stored_block_ref_count(&path, leaf_bid(5)?)?, 6);
        assert_eq!(stored_block_ref_count(&path, leaf_bid(9)?)?, 8);

        let store = open_store(&path)?;
        let ipm = store.open_folder(
            &store
                .properties()
                .make_entry_id(node(NodeIdType::NormalFolder, IPM_FOLDER_INDEX)?)?,
        )?;
        let ipm_hierarchy = ipm.hierarchy_table().ok_or("missing IPM hierarchy")?;
        assert_eq!(ipm_hierarchy.rows_matrix().count(), 4);
        let deleted_node = node(NodeIdType::NormalFolder, DELETED_FOLDER_INDEX)?;
        let deleted = store.open_folder(&store.properties().make_entry_id(deleted_node)?)?;
        assert_eq!(deleted.properties().display_name()?, "Deleted Items");
        assert_eq!(
            deleted
                .contents_table()
                .ok_or("missing Deleted Items contents")?
                .rows_matrix()
                .count(),
            1
        );
        let user_deleted_node = node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX + 2)?;
        let user_deleted =
            store.open_folder(&store.properties().make_entry_id(user_deleted_node)?)?;
        assert_eq!(user_deleted.properties().display_name()?, "Deleted items");
        assert_eq!(
            user_deleted
                .contents_table()
                .ok_or("missing user-created Deleted items contents")?
                .rows_matrix()
                .count(),
            1
        );
        let inbox_node = node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX + 3)?;
        let inbox = store.open_folder(&store.properties().make_entry_id(inbox_node)?)?;
        assert_eq!(inbox.properties().display_name()?, "Inbox");
        assert_eq!(
            inbox
                .contents_table()
                .ok_or("missing Inbox contents")?
                .rows_matrix()
                .count(),
            0
        );
        let children = inbox.hierarchy_table().ok_or("missing Inbox hierarchy")?;
        assert_eq!(children.rows_matrix().count(), 2);
        for index in [MAIL_FOLDER_INDEX + 4, MAIL_FOLDER_INDEX + 5] {
            children.find_row(crate::ltp::table_context::TableRowId::new(u32::from(node(
                NodeIdType::NormalFolder,
                index,
            )?)))?;
        }

        Ok(())
    }

    #[test]
    fn explicit_deleted_items_children_do_not_count_default_shared_tables()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("deleted-children.pst");
        let base = FidelityStore::default();
        let message = |subject: &str| MessageSpec {
            subject: subject.to_owned(),
            ..base.message.clone()
        };
        let mut spec = MailStoreSpec {
            store_name: "PSTForge Deleted Items hierarchy".to_owned(),
            record_key: base.record_key,
            folders: vec![
                MailFolderSpec {
                    path: vec!["Deleted Items".to_owned()],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::DeletedItems,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![message("deleted")],
                    associated_messages: Vec::new(),
                },
                MailFolderSpec {
                    path: vec!["Deleted Items".to_owned(), "Child".to_owned()],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![message("deleted child")],
                    associated_messages: Vec::new(),
                },
                MailFolderSpec {
                    path: vec!["Inbox".to_owned()],
                    location: MailFolderLocation::IpmSubtree,
                    role: MailFolderRole::Ordinary,
                    container_class: "IPF.Note".to_owned(),
                    messages: vec![message("inbox")],
                    associated_messages: Vec::new(),
                },
            ],
        };
        create_mail_store(&path, &spec)?;
        assert_eq!(stored_block_ref_count(&path, leaf_bid(5)?)?, 5);
        assert_eq!(stored_block_ref_count(&path, leaf_bid(9)?)?, 5);

        spec.folders[0].messages.clear();
        let empty_deleted_path = directory.path().join("empty-deleted-with-child.pst");
        create_mail_store(&empty_deleted_path, &spec)?;
        assert_eq!(
            stored_block_ref_count(&empty_deleted_path, leaf_bid(5)?)?,
            6
        );
        assert_eq!(
            stored_block_ref_count(&empty_deleted_path, leaf_bid(9)?)?,
            5
        );
        Ok(())
    }

    fn stored_block_ref_count(
        path: &Path,
        block: UnicodeBlockId,
    ) -> Result<u16, Box<dyn std::error::Error>> {
        let pst = UnicodePstFile::open(path)?;
        let root = pst.header().root();
        let mut reader = pst.reader().lock().map_err(|_| "reader lock failed")?;
        let bbt = crate::ndb::page::UnicodeBlockBTree::read(&mut *reader, *root.block_btree())?;
        let mut cache = Default::default();
        Ok(bbt
            .find_entry(&mut *reader, block.search_key(), &mut cache)?
            .ref_count())
    }

    #[test]
    fn fidelity_store_round_trips_rich_mail() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("fidelity.pst");
        let mut spec = FidelityStore::default();
        spec.message.message_flags = 0x02;
        create_fidelity_store(&path, &spec)?;

        let mut header = std::fs::File::open(&path)?;
        header.seek(SeekFrom::Start(44))?;
        let mut counters = [0_u32; 32];
        for counter in &mut counters {
            *counter = byteorder::ReadBytesExt::read_u32::<LittleEndian>(&mut header)?;
        }
        assert_eq!(counters[NodeIdType::NormalMessage as usize], 0x3_0001);
        assert_eq!(counters[NodeIdType::Attachment as usize], 0x2_0001);
        assert_eq!(
            counters[NodeIdType::ListsTablesProperties as usize],
            0x4_0000
        );

        let store = open_store(&path)?;
        let message_node = node(NodeIdType::NormalMessage, MESSAGE_INDEX)?;
        let message = store.open_message(
            &EntryId::new(
                crate::messaging::store::StoreRecordKey::new(spec.record_key),
                message_node,
            ),
            None,
        )?;
        assert_eq!(message.properties().message_class()?, "IPM.Note");
        assert!(matches!(
            message.properties().get(0x0037),
            Some(crate::ltp::prop_context::PropertyValue::Unicode(value))
                if value.to_string() == spec.message.subject
        ));
        assert!(matches!(
            message.properties().get(0x0E07),
            Some(crate::ltp::prop_context::PropertyValue::Integer32(0x12))
        ));
        assert!(matches!(
            message.properties().get(0x1013),
            Some(crate::ltp::prop_context::PropertyValue::Binary(value))
                if value.buffer() == spec.message.body_html.as_deref().unwrap_or_default()
        ));
        assert!(matches!(
            message.properties().get(0x3FDE),
            Some(crate::ltp::prop_context::PropertyValue::Integer32(65001))
        ));
        assert!(matches!(
            message.properties().get(0x1016),
            Some(crate::ltp::prop_context::PropertyValue::Integer32(3))
        ));
        assert!(message.properties().get(0x3FFD).is_none());
        assert_eq!(
            message
                .recipient_table()
                .ok_or("missing recipient table")?
                .rows_matrix()
                .count(),
            3
        );
        assert_eq!(
            message
                .attachment_table()
                .ok_or("missing attachment table")?
                .rows_matrix()
                .count(),
            2
        );
        assert!(message.properties().get(0x8000).is_some());

        use crate::messaging::{
            attachment::{Attachment, AttachmentData, UnicodeAttachment},
            message::UnicodeMessage,
            read_write::MessageReadWrite,
            store::{Store, UnicodeStore},
        };
        use std::rc::Rc;
        let pst = Rc::new(UnicodePstFile::open(&path)?);
        let concrete_store = UnicodeStore::read(pst)?;
        let concrete_message = UnicodeMessage::read(
            concrete_store.clone(),
            &EntryId::new(
                crate::messaging::store::StoreRecordKey::new(spec.record_key),
                message_node,
            ),
            None,
        )?;
        let binary_node = node(NodeIdType::Attachment, 0x2_0000)?;
        let binary_entry = concrete_message
            .sub_nodes()
            .get(&binary_node)
            .ok_or("missing binary attachment subnode")?;
        assert!(binary_entry.sub_node().is_some());
        let binary = UnicodeAttachment::read(concrete_message.clone(), binary_node, None)
            .map_err(|error| format!("binary attachment read failed: {error}"))?;
        let expected_binary = match &spec.message.attachments[0].content {
            AttachmentContent::Binary(value) => value,
            AttachmentContent::Embedded(_) | AttachmentContent::Spooled(_) => {
                return Err("expected binary attachment".into());
            }
        };
        assert!(matches!(
            binary.data(),
            Some(AttachmentData::Binary(value))
                if value.buffer() == expected_binary
        ));
        assert_eq!(binary.properties().attachment_size()?, 16_546);

        let embedded_node = node(NodeIdType::Attachment, 0x2_0001)?;
        let embedded = UnicodeAttachment::read(concrete_message, embedded_node, None)
            .map_err(|error| format!("embedded attachment read failed: {error}"))?;
        let embedded_message = match embedded.data() {
            Some(AttachmentData::Message(message)) => message,
            _ => return Err("expected embedded message attachment".into()),
        };
        let embedded_message_size = embedded_message.properties().message_size()?;
        assert!(matches!(
            embedded_message.properties().get(0x3FDE),
            Some(crate::ltp::prop_context::PropertyValue::Integer32(65001))
        ));
        assert!(matches!(
            embedded_message.properties().get(0x1016),
            Some(crate::ltp::prop_context::PropertyValue::Integer32(1))
        ));
        assert!(embedded_message.properties().get(0x3FFD).is_none());
        assert!(matches!(
            embedded.properties().get(0x3701),
            Some(crate::ltp::prop_context::PropertyValue::Object(value))
                if value.size() == u32::try_from(embedded_message_size)?
        ));
        let expected_embedded = &spec.message.attachments[1];
        let expected_embedded_size = attachment_property_size(&attachment_properties(
            expected_embedded,
            1,
            5,
            0,
            PropertyValue::Object(
                node(NodeIdType::NormalMessage, 0x3_0001)?,
                u32::try_from(embedded_message_size)?,
            ),
        ))?;
        assert_eq!(
            embedded.properties().attachment_size()?,
            expected_embedded_size
        );
        let named = concrete_store.named_property_map()?;
        assert_eq!(named.properties().stream_entry()?.len(), 3);
        Ok(())
    }

    #[test]
    fn header_crc_rejects_tampering() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("tampered.pst");
        create_minimal_store(&path, &MinimalStore::default())?;
        let mut file = OpenOptions::new().write(true).open(&path)?;
        file.seek(SeekFrom::Start(20))?;
        file.write_all(&[0xAA])?;
        file.sync_all()?;
        assert!(UnicodePstFile::open(&path).is_err());
        Ok(())
    }

    #[test]
    fn create_refuses_existing_output() -> Result<(), Box<dyn std::error::Error>> {
        let file = tempfile::NamedTempFile::new()?;
        let error = create_minimal_store(file.path(), &MinimalStore::default())
            .expect_err("existing output must be refused");
        assert!(matches!(error, WriterError::OutputExists(_)));
        Ok(())
    }

    #[test]
    fn atomic_publish_refuses_existing_destination() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("existing.pst");
        std::fs::write(&destination, b"existing")?;
        let destination_directory = std::fs::File::open(directory.path())?;
        let mut temporary = PublicationTemporary::new(directory.path())?;
        temporary.file.write_all(b"replacement")?;
        temporary.file.sync_all()?;

        let error = publish_noclobber(
            temporary.source_name(),
            &temporary.directory,
            &destination_directory,
            &destination,
        )
        .expect_err("atomic publication must not replace an existing destination");
        assert!(matches!(error, WriterError::OutputExists(path) if path == destination));
        assert_eq!(std::fs::read(&destination)?, b"existing");
        assert!(
            temporary
                .directory_path()?
                .join(temporary.source_name())
                .exists()
        );
        Ok(())
    }

    #[test]
    fn durability_error_reports_already_published_output() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("published.pst");
        let destination_directory = std::fs::File::open(directory.path())?;
        let mut temporary = PublicationTemporary::new(directory.path())?;
        temporary.file.write_all(b"published")?;
        temporary.file.sync_all()?;
        publish_noclobber(
            temporary.source_name(),
            &temporary.directory,
            &destination_directory,
            &destination,
        )?;

        let unsyncable = std::fs::File::open("/proc/self/status")?;
        let error = sync_published_directory(&destination, &unsyncable)
            .expect_err("sync failure must report uncertain publication durability");
        assert!(matches!(
            error,
            WriterError::PublishedDurability { path, .. } if path == destination
        ));
        assert_eq!(std::fs::read(&destination)?, b"published");
        assert!(
            !temporary
                .directory_path()?
                .join(temporary.source_name())
                .exists()
        );
        Ok(())
    }

    #[test]
    fn publication_uses_held_source_directory() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("published.pst");
        let mut temporary = PublicationTemporary::new(directory.path())?;
        let original_source_path = temporary.directory_path()?;
        let moved_source_path = directory.path().join("moved-source");
        let destination_directory = std::fs::File::open(directory.path())?;
        temporary.file.write_all(b"validated")?;
        temporary.file.sync_all()?;
        let source_name = temporary.source_name().to_owned();

        std::fs::rename(&original_source_path, &moved_source_path)?;
        std::fs::create_dir(&original_source_path)?;
        let replacement = original_source_path.join(&source_name);
        std::fs::write(&replacement, b"replacement")?;

        publish_noclobber(
            temporary.source_name(),
            &temporary.directory,
            &destination_directory,
            &destination,
        )?;
        assert_eq!(std::fs::read(&destination)?, b"validated");
        drop(temporary);
        assert_eq!(std::fs::read(replacement)?, b"replacement");
        Ok(())
    }

    #[test]
    fn cleanup_preserves_empty_stale_path_replacement() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let temporary = PublicationTemporary::new(directory.path())?;
        let original_source_path = temporary.directory_path()?;
        let moved_source_path = directory.path().join("moved-source");
        std::fs::rename(&original_source_path, &moved_source_path)?;
        std::fs::create_dir(&original_source_path)?;

        drop(temporary);
        assert!(original_source_path.is_dir());
        Ok(())
    }

    #[test]
    fn validator_timeout_terminates_and_captures_bounded_diagnostics()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut command = Command::new("sh");
        command.args(["-c", "printf checkpoint; (sleep 5) >&1 2>&2 & exit 0"]);
        let started = Instant::now();
        let output = run_validator(&mut command, Duration::from_millis(25), &NEVER_INTERRUPTED)?;
        assert!(output.timed_out);
        assert!(!output.success);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(output.stdout, b"checkpoint");

        let input = vec![b'x'; MAX_VALIDATOR_DIAGNOSTIC_BYTES + 1];
        let (captured, truncated) = capture_bounded(input.as_slice())?;
        assert_eq!(captured.len(), MAX_VALIDATOR_DIAGNOSTIC_BYTES);
        assert!(truncated);
        Ok(())
    }

    #[test]
    fn validator_scratch_stays_beneath_publication_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let temporary = PublicationTemporary::new(directory.path())?;
        let publication = temporary.directory_path()?.canonicalize()?;
        let scratch = temporary.validator_scratch()?;
        assert!(scratch.path().canonicalize()?.starts_with(publication));
        Ok(())
    }

    #[test]
    fn validator_interruption_terminates_its_process_group()
    -> Result<(), Box<dyn std::error::Error>> {
        let interrupted = AtomicBool::new(true);
        let mut command = Command::new("sh");
        command.args(["-c", "(sleep 30) >&1 2>&2 & wait"]);
        let started = Instant::now();
        let error = match run_validator(&mut command, Duration::from_secs(60), &interrupted) {
            Ok(_) => return Err("the validator ignored the interruption".into()),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(2));
        Ok(())
    }

    #[test]
    fn rejected_validator_candidate_and_diagnostics_are_retained()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut temporary = PublicationTemporary::new(directory.path())?;
        temporary.file.write_all(b"unpublished candidate")?;
        temporary.file.sync_all()?;
        let source_name = temporary.source_name().to_owned();
        let output = ValidatorOutput {
            success: false,
            timed_out: true,
            stdout: b"bounded stdout".to_vec(),
            stderr: b"bounded stderr".to_vec(),
            stdout_truncated: false,
            stderr_truncated: false,
        };
        let evidence = temporary.retain_validation_failure("fixture", &output)?;
        drop(temporary);

        assert_eq!(
            std::fs::read(evidence.join(source_name))?,
            b"unpublished candidate"
        );
        let diagnostics = std::fs::read_to_string(evidence.join("validator-failure.log"))?;
        assert!(diagnostics.contains("tool: fixture"));
        assert!(diagnostics.contains("timed out: true"));
        assert!(diagnostics.contains("validator output redacted"));
        assert!(!diagnostics.contains("bounded stdout"));
        assert!(!diagnostics.contains("bounded stderr"));
        Ok(())
    }

    #[test]
    fn publication_reports_moved_destination_directory() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let parent = root.path().join("destination");
        let moved_parent = root.path().join("moved-destination");
        std::fs::create_dir(&parent)?;
        let destination = parent.join("published.pst");
        let destination_directory = std::fs::File::open(&parent)?;
        let mut temporary = PublicationTemporary::new(&parent)?;
        temporary.file.write_all(b"validated")?;
        temporary.file.sync_all()?;

        std::fs::rename(&parent, &moved_parent)?;
        std::fs::create_dir(&parent)?;
        publish_noclobber(
            temporary.source_name(),
            &temporary.directory,
            &destination_directory,
            &destination,
        )?;
        sync_published_directory(&destination, &destination_directory)?;

        let error = verify_published_destination(&destination, &temporary.file)
            .expect_err("moved destination parent must make publication uncertain");
        assert!(matches!(
            error,
            WriterError::PublishedDestinationChanged(path) if path == destination
        ));
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read(moved_parent.join("published.pst"))?,
            b"validated"
        );
        Ok(())
    }

    #[test]
    fn repeated_writes_are_byte_identical() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let first = directory.path().join("first.pst");
        let second = directory.path().join("second.pst");
        let spec = MinimalStore::default();
        create_minimal_store(&first, &spec)?;
        create_minimal_store(&second, &spec)?;
        assert_eq!(std::fs::read(first)?, std::fs::read(second)?);
        Ok(())
    }

    #[test]
    fn repeated_fidelity_writes_are_byte_identical() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let first = directory.path().join("first-rich.pst");
        let second = directory.path().join("second-rich.pst");
        let spec = FidelityStore::default();
        create_fidelity_store(&first, &spec)?;
        create_fidelity_store(&second, &spec)?;
        assert_eq!(std::fs::read(first)?, std::fs::read(second)?);
        Ok(())
    }

    #[test]
    fn named_property_map_orders_numeric_and_string_identities()
    -> Result<(), Box<dyn std::error::Error>> {
        let properties = [
            NamedProperty {
                set: NamedPropertySet::PublicStrings,
                name: NamedPropertyName::String("PSTForge Name".to_owned()),
                value: RawPropertyValue::Unicode("string value".to_owned()),
            },
            NamedProperty {
                set: NamedPropertySet::Mapi,
                name: NamedPropertyName::Numeric(0x8005),
                value: RawPropertyValue::Integer32(7),
            },
        ];
        let mut identities = properties
            .iter()
            .map(|property| (property.set, property.name.clone()))
            .collect::<Vec<_>>();
        identities.sort();
        let map = named_property_map(&identities)?;
        let entries = map
            .iter()
            .find(|(id, _)| *id == 0x0003)
            .ok_or("missing named-property entry stream")?;
        assert!(matches!(&entries.1, PropertyValue::Binary(bytes) if bytes.len() == 16));
        let strings = map
            .iter()
            .find(|(id, _)| *id == 0x0004)
            .ok_or("missing named-property string stream")?;
        assert!(matches!(&strings.1, PropertyValue::Binary(bytes) if !bytes.is_empty()));
        assert_eq!(map.iter().filter(|(id, _)| *id >= 0x1000).count(), 2);
        Ok(())
    }

    #[test]
    fn empty_named_property_map_preserves_required_interoperability_streams()
    -> Result<(), Box<dyn std::error::Error>> {
        let map = named_property_map(&[])?;
        assert!(matches!(
            map.iter().find(|(id, _)| *id == 0x0003),
            Some((_, PropertyValue::Binary(entries))) if entries.len() == 8
        ));
        assert!(matches!(
            map.iter().find(|(id, _)| *id == 0x1000),
            Some((_, PropertyValue::Binary(entries))) if entries.len() == 8
        ));
        Ok(())
    }

    #[test]
    fn fidelity_validation_handles_empty_raw_values_and_rejects_ambiguous_inputs()
    -> Result<(), Box<dyn std::error::Error>> {
        let result = std::thread::Builder::new()
            .name("pstforge-writer-validation-test".to_owned())
            .stack_size(WRITER_STACK_BYTES)
            .spawn(|| {
                fidelity_validation_handles_empty_raw_values_and_rejects_ambiguous_inputs_inner()
                    .map_err(|error| error.to_string())
            })?
            .join()
            .map_err(|_| "writer validation test thread panicked")?;
        result.map_err(Into::into)
    }

    fn fidelity_validation_handles_empty_raw_values_and_rejects_ambiguous_inputs_inner()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut empty_body = FidelityStore::default();
        empty_body.message.body_text = Some(String::new());
        assert!(matches!(
            validate_spec(&empty_body),
            Err(WriterError::InvalidStructure(_))
        ));

        let mut empty_raw = FidelityStore::default();
        empty_raw.message.raw_properties.push(RawProperty {
            id: 0x1101,
            value: RawPropertyValue::Binary(Vec::new()),
        });
        empty_raw.message.raw_properties.push(RawProperty {
            id: 0x1102,
            value: RawPropertyValue::Unicode(String::new()),
        });
        validate_spec(&empty_raw)?;

        let mut empty_spooled_property = FidelityStore::default();
        empty_spooled_property
            .message
            .spooled_properties
            .push(SpooledPropertySpec {
                id: 0x1103,
                property_type: 0x0102,
                blob: FileBlobSpec {
                    path: PathBuf::from("/dev/null"),
                    offset: 0,
                    byte_len: 0,
                    sha256: Sha256::digest([]).into(),
                },
            });
        assert!(matches!(
            validate_spec(&empty_spooled_property),
            Err(WriterError::InvalidStructure(detail))
                if detail.contains("streamed property blob must be non-empty")
        ));

        let mut empty_spooled_attachment = FidelityStore::default();
        empty_spooled_attachment.message.attachments[0].content =
            AttachmentContent::Spooled(FileBlobSpec {
                path: PathBuf::from("/dev/null"),
                offset: 0,
                byte_len: 0,
                sha256: Sha256::digest([]).into(),
            });
        assert!(matches!(
            validate_spec(&empty_spooled_attachment),
            Err(WriterError::InvalidStructure(detail))
                if detail.contains("spooled attachment payload must be non-empty")
        ));

        let mut invalid_html = FidelityStore::default();
        invalid_html.message.body_html = Some(vec![0xFF]);
        assert!(matches!(
            validate_spec(&invalid_html),
            Err(WriterError::InvalidStructure(_))
        ));

        let mut wrong_contents_type = FidelityStore::default();
        wrong_contents_type
            .message
            .raw_properties
            .push(RawProperty {
                id: 0x0017,
                value: RawPropertyValue::Unicode("not an integer".to_owned()),
            });
        assert!(matches!(
            validate_spec(&wrong_contents_type),
            Err(WriterError::InvalidStructure(detail))
                if detail.contains("expected Integer32")
        ));

        let mut invalid_embedded_html = FidelityStore::default();
        if let AttachmentContent::Embedded(message) =
            &mut invalid_embedded_html.message.attachments[1].content
        {
            message.body_html = Some(vec![0xFF]);
        }
        assert!(matches!(
            validate_spec(&invalid_embedded_html),
            Err(WriterError::InvalidStructure(_))
        ));

        for (native_body, clear_body) in [
            (NativeBody::PlainText, 1),
            (NativeBody::Rtf, 2),
            (NativeBody::Html, 3),
        ] {
            let mut missing_native_body = FidelityStore::default();
            missing_native_body.message.native_body = Some(native_body);
            match clear_body {
                1 => missing_native_body.message.body_text = None,
                2 => missing_native_body.message.body_rtf = None,
                3 => missing_native_body.message.body_html = None,
                _ => unreachable!(),
            }
            assert!(matches!(
                validate_spec(&missing_native_body),
                Err(WriterError::InvalidStructure(_))
            ));
        }

        let mut top_rtf_sync_without_rtf = FidelityStore::default();
        top_rtf_sync_without_rtf.message.body_rtf = None;
        top_rtf_sync_without_rtf.message.rtf_in_sync = true;
        assert!(matches!(
            validate_spec(&top_rtf_sync_without_rtf),
            Err(WriterError::InvalidStructure(_))
        ));

        let mut embedded_rtf_sync_without_rtf = FidelityStore::default();
        if let AttachmentContent::Embedded(message) =
            &mut embedded_rtf_sync_without_rtf.message.attachments[1].content
        {
            message.body_rtf = None;
            message.rtf_in_sync = true;
        }
        assert!(matches!(
            validate_spec(&embedded_rtf_sync_without_rtf),
            Err(WriterError::InvalidStructure(_))
        ));

        let mut raw_collision = FidelityStore::default();
        raw_collision.message.raw_properties[0].id = 0x3FDE;
        assert!(matches!(
            validate_spec(&raw_collision),
            Err(WriterError::InvalidStructure(_))
        ));

        let mut native_body_collision = FidelityStore::default();
        native_body_collision.message.raw_properties[0].id = 0x1016;
        assert!(matches!(
            validate_spec(&native_body_collision),
            Err(WriterError::InvalidStructure(_))
        ));

        let mut duplicate_named = FidelityStore::default();
        duplicate_named
            .message
            .named_properties
            .push(duplicate_named.message.named_properties[0].clone());
        assert!(matches!(
            validate_spec(&duplicate_named),
            Err(WriterError::InvalidStructure(_))
        ));

        let mut nested = FidelityStore::default();
        if let AttachmentContent::Embedded(message) = &mut nested.message.attachments[1].content {
            message.attachments.push(AttachmentSpec {
                filename: "nested.bin".to_owned(),
                mime_type: None,
                content_id: None,
                content_location: None,
                rendering_position: None,
                flags: 0,
                raw_properties: Vec::new(),
                content: AttachmentContent::Binary(vec![1]),
            });
        }
        let directory = tempfile::tempdir()?;
        let nested_path = directory.path().join("nested.pst");
        create_fidelity_store(&nested_path, &nested)?;
        assert!(nested_path.is_file());
        let top_key = message_record_key(
            nested.record_key,
            node(NodeIdType::NormalMessage, MESSAGE_INDEX)?,
        );
        let embedded_key = embedded_message_record_key(top_key, 1);
        let nested_key = embedded_message_record_key(embedded_key, 1);
        assert_ne!(top_key, embedded_key);
        assert_ne!(embedded_key, nested_key);
        assert_ne!(top_key, nested_key);

        let template = match &FidelityStore::default().message.attachments[1].content {
            AttachmentContent::Embedded(message) => {
                let mut message = message.as_ref().clone();
                message.attachments.clear();
                message
            }
            AttachmentContent::Binary(_) | AttachmentContent::Spooled(_) => {
                return Err("expected embedded fixture".into());
            }
        };
        let mut child = template.clone();
        for _ in 0..=MAX_EMBEDDED_MESSAGE_DEPTH {
            let mut parent = template.clone();
            parent.attachments.push(AttachmentSpec {
                filename: "nested.msg".to_owned(),
                mime_type: Some("message/rfc822".to_owned()),
                content_id: None,
                content_location: None,
                rendering_position: None,
                flags: 0,
                raw_properties: Vec::new(),
                content: AttachmentContent::Embedded(Box::new(child)),
            });
            child = parent;
        }
        let too_deep = FidelityStore {
            message: child,
            ..FidelityStore::default()
        };
        assert!(matches!(
            validate_spec(&too_deep),
            Err(WriterError::ValueTooLarge("embedded message depth"))
        ));

        let mut embedded_collision = FidelityStore::default();
        if let AttachmentContent::Embedded(message) =
            &mut embedded_collision.message.attachments[1].content
        {
            message.raw_properties.push(RawProperty {
                id: 0x3FDE,
                value: RawPropertyValue::Integer32(1252),
            });
        }
        assert!(matches!(
            validate_spec(&embedded_collision),
            Err(WriterError::InvalidStructure(_))
        ));

        let mut embedded_native_body_collision = FidelityStore::default();
        if let AttachmentContent::Embedded(message) =
            &mut embedded_native_body_collision.message.attachments[1].content
        {
            message.raw_properties.push(RawProperty {
                id: 0x1016,
                value: RawPropertyValue::Integer32(1),
            });
        }
        assert!(matches!(
            validate_spec(&embedded_native_body_collision),
            Err(WriterError::InvalidStructure(_))
        ));
        Ok(())
    }

    #[test]
    fn native_body_values_and_absence_are_explicit() -> Result<(), Box<dyn std::error::Error>> {
        let mut spec = FidelityStore::default();
        let identities = collect_named_identities(&spec.message);
        for (native_body, expected) in [
            (NativeBody::PlainText, 1),
            (NativeBody::Rtf, 2),
            (NativeBody::Html, 3),
        ] {
            spec.message.native_body = Some(native_body);
            let properties = message_properties(&spec.message, false, &identities, [0; 16], 0)?;
            assert!(matches!(
                properties.iter().find(|(id, _)| *id == 0x1016),
                Some((_, PropertyValue::Integer32(actual))) if *actual == expected
            ));
        }
        spec.message.native_body = None;
        let properties = message_properties(&spec.message, false, &identities, [0; 16], 0)?;
        assert!(properties.iter().all(|(id, _)| *id != 0x1016));
        Ok(())
    }

    #[test]
    fn fidelity_writer_bounds_generated_aggregate_properties()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let recipient = |length| RecipientSpec {
            kind: RecipientKind::To,
            display_name: "R".repeat(length),
            email_address: "r@example.com".to_owned(),
        };
        let mut recipients_at_limit = FidelityStore::default();
        recipients_at_limit.message.recipients = vec![recipient(2048)];
        create_fidelity_store(
            directory.path().join("recipient-limit.pst"),
            &recipients_at_limit,
        )?;
        recipients_at_limit.message.recipients.push(recipient(2048));
        assert!(matches!(
            create_fidelity_store(
                directory.path().join("recipient-overflow.pst"),
                &recipients_at_limit
            ),
            Err(WriterError::ValueTooLarge("recipient table heap page"))
        ));

        let property = |name: String| NamedProperty {
            set: NamedPropertySet::PublicStrings,
            name: NamedPropertyName::String(name),
            value: RawPropertyValue::Boolean(true),
        };
        let mut names_at_limit = FidelityStore::default();
        names_at_limit.message.named_properties.clear();
        if let AttachmentContent::Embedded(message) =
            &mut names_at_limit.message.attachments[1].content
        {
            message.named_properties.clear();
        }
        names_at_limit.message.named_properties = vec![property(format!("A{}", "x".repeat(2047)))];
        create_fidelity_store(directory.path().join("nameid-limit.pst"), &names_at_limit)?;
        names_at_limit
            .message
            .named_properties
            .push(property(format!("B{}", "x".repeat(2047))));
        assert!(matches!(
            create_fidelity_store(
                directory.path().join("nameid-overflow.pst"),
                &names_at_limit
            ),
            Err(WriterError::ValueTooLarge("named-property map heap page"))
        ));

        let mut raw_overflow = FidelityStore::default();
        raw_overflow.message.raw_properties = (0..5)
            .map(|index| RawProperty {
                id: 0x1100 + index,
                value: RawPropertyValue::Binary(vec![0; MAX_FIDELITY_PROPERTY_BYTES]),
            })
            .collect();
        assert!(matches!(
            create_fidelity_store(directory.path().join("raw-overflow.pst"), &raw_overflow),
            Err(WriterError::ValueTooLarge(
                "aggregate custom-property payload"
            ))
        ));

        let mut guid_at_limit = FidelityStore::default();
        guid_at_limit.message.raw_properties = vec![RawProperty {
            id: 0x1100,
            value: RawPropertyValue::MultipleGuid(vec![
                [0xAB; 16];
                MAX_FIDELITY_PROPERTY_BYTES / 16
            ]),
        }];
        create_fidelity_store(directory.path().join("guid-limit.pst"), &guid_at_limit)?;
        if let RawPropertyValue::MultipleGuid(values) =
            &mut guid_at_limit.message.raw_properties[0].value
        {
            values.push([0xCD; 16]);
        }
        assert!(matches!(
            create_fidelity_store(directory.path().join("guid-overflow.pst"), &guid_at_limit),
            Err(WriterError::ValueTooLarge("raw property"))
        ));

        let mut unsupported_overflow = FidelityStore::default();
        unsupported_overflow.message.unsupported_properties = vec![
            UnsupportedProperty {
                id: 0x1234,
                property_type: 0x101F,
                byte_len: 1,
            };
            MAX_FIDELITY_COLLECTION_ITEMS + 1
        ];
        assert!(matches!(
            create_fidelity_store(
                directory.path().join("unsupported-overflow.pst"),
                &unsupported_overflow
            ),
            Err(WriterError::ValueTooLarge("unsupported-property count"))
        ));

        let mut attachment_overflow = FidelityStore::default();
        attachment_overflow.message.attachments = (0..3)
            .map(|index| AttachmentSpec {
                filename: format!("{index}{}", "x".repeat(2047)),
                mime_type: None,
                content_id: None,
                content_location: None,
                rendering_position: None,
                flags: 0,
                raw_properties: Vec::new(),
                content: AttachmentContent::Binary(Vec::new()),
            })
            .collect();
        assert!(matches!(
            create_fidelity_store(
                directory.path().join("attachment-table-overflow.pst"),
                &attachment_overflow
            ),
            Err(WriterError::ValueTooLarge("attachment table heap page"))
        ));

        let scalar_properties = |count| {
            (0..count)
                .map(|index| RawProperty {
                    id: 0x1100 + index,
                    value: RawPropertyValue::Boolean(true),
                })
                .collect::<Vec<_>>()
        };
        let scalar_boundary = (0..1000_u16)
            .find(|count| {
                let mut candidate = FidelityStore::default();
                candidate.message.raw_properties = scalar_properties(*count);
                validate_spec(&candidate).is_err()
            })
            .and_then(|count| count.checked_sub(1))
            .ok_or("message property-context boundary was not found")?;
        let mut scalar_at_limit = FidelityStore::default();
        scalar_at_limit.message.raw_properties = scalar_properties(scalar_boundary);
        create_fidelity_store(
            directory.path().join("message-pc-limit.pst"),
            &scalar_at_limit,
        )?;
        let mut scalar_overflow = FidelityStore::default();
        scalar_overflow.message.raw_properties = scalar_properties(scalar_boundary + 1);
        assert!(matches!(
            create_fidelity_store(
                directory.path().join("message-pc-overflow.pst"),
                &scalar_overflow
            ),
            Err(WriterError::ValueTooLarge("message property context"))
        ));

        let mut empty_attachment = FidelityStore::default();
        if let AttachmentContent::Binary(data) =
            &mut empty_attachment.message.attachments[0].content
        {
            data.clear();
        }
        create_fidelity_store(
            directory.path().join("empty-attachment.pst"),
            &empty_attachment,
        )?;
        Ok(())
    }

    #[test]
    fn fidelity_external_property_boundary_and_accounting() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let mut spec = FidelityStore::default();
        spec.message
            .unsupported_properties
            .push(UnsupportedProperty {
                id: 0x1234,
                property_type: 0x101F,
                byte_len: 50_000,
            });
        if let AttachmentContent::Embedded(embedded) = &mut spec.message.attachments[1].content {
            embedded
                .unsupported_properties
                .push(spec.message.unsupported_properties[0].clone());
        }
        let report = create_fidelity_store(directory.path().join("bounded.pst"), &spec)?;
        assert_eq!(report.unsupported_properties.len(), 2);
        assert!(report.unsupported_properties[0].message_path.is_empty());
        assert_eq!(
            report.unsupported_properties[0].property,
            spec.message.unsupported_properties[0]
        );
        assert_eq!(report.unsupported_properties[1].message_path, [1]);
        assert_eq!(
            report.unsupported_properties[1].property,
            spec.message.unsupported_properties[0]
        );

        let mut too_large = FidelityStore::default();
        if let AttachmentContent::Binary(data) = &mut too_large.message.attachments[0].content {
            data.push(0);
        }
        assert!(matches!(
            create_fidelity_store(directory.path().join("too-large.pst"), &too_large),
            Err(WriterError::ValueTooLarge(_))
        ));
        Ok(())
    }

    #[test]
    fn embedded_named_properties_share_the_store_mapping() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut spec = FidelityStore::default();
        match &mut spec.message.attachments[1].content {
            AttachmentContent::Embedded(message) => {
                message.named_properties.push(NamedProperty {
                    set: NamedPropertySet::PublicStrings,
                    name: NamedPropertyName::String("EmbeddedName".to_owned()),
                    value: RawPropertyValue::Boolean(true),
                });
            }
            AttachmentContent::Binary(_) | AttachmentContent::Spooled(_) => {
                return Err("expected embedded fixture".into());
            }
        }
        let identities = collect_named_identities(&spec.message);
        let embedded = match &spec.message.attachments[1].content {
            AttachmentContent::Embedded(message) => message,
            AttachmentContent::Binary(_) | AttachmentContent::Spooled(_) => {
                return Err("expected embedded fixture".into());
            }
        };
        assert_eq!(identities.len(), 4);
        let properties = message_properties(
            embedded,
            false,
            &identities,
            embedded_message_record_key(
                message_record_key(
                    spec.record_key,
                    node(NodeIdType::NormalMessage, MESSAGE_INDEX)?,
                ),
                1,
            ),
            0,
        )?;
        let expected_index = identities
            .binary_search(&(
                NamedPropertySet::PublicStrings,
                NamedPropertyName::String("EmbeddedName".to_owned()),
            ))
            .map_err(|_| "embedded named property was not mapped")?;
        assert!(properties.iter().any(|(id, value)| {
            *id >= 0x8000
                && usize::from(*id - 0x8000) == expected_index
                && matches!(value, PropertyValue::Boolean(true))
        }));
        Ok(())
    }

    #[test]
    fn calendar_exception_attachment_properties_round_trip()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut spec = FidelityStore::default();
        spec.message.message_class = "IPM.Appointment".to_owned();
        let attachment = &mut spec.message.attachments[1];
        let AttachmentContent::Embedded(message) = &mut attachment.content else {
            return Err("expected embedded fixture".into());
        };
        message.message_class = "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}".to_owned();
        attachment.raw_properties = vec![
            RawProperty {
                id: 0x3001,
                value: RawPropertyValue::Unicode("exception".to_owned()),
            },
            RawProperty {
                id: 0x3702,
                value: RawPropertyValue::Binary(Vec::new()),
            },
            RawProperty {
                id: 0x3709,
                value: RawPropertyValue::Binary(vec![1, 2, 3, 4]),
            },
            RawProperty {
                id: 0x7FFA,
                value: RawPropertyValue::Integer32(0),
            },
            RawProperty {
                id: 0x7FFB,
                value: RawPropertyValue::Time(133_815_132_000_000_000),
            },
            RawProperty {
                id: 0x7FFC,
                value: RawPropertyValue::Time(133_815_168_000_000_000),
            },
            RawProperty {
                id: 0x7FFD,
                value: RawPropertyValue::Integer32(2),
            },
            RawProperty {
                id: 0x7FFE,
                value: RawPropertyValue::Boolean(true),
            },
            RawProperty {
                id: 0x7FFF,
                value: RawPropertyValue::Boolean(false),
            },
        ];

        let directory = tempfile::tempdir()?;
        create_fidelity_store(directory.path().join("calendar-exception.pst"), &spec)?;

        let mut top_level = FidelityStore::default();
        top_level.message.message_class =
            "IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}".to_owned();
        assert!(matches!(
            validate_spec(&top_level),
            Err(WriterError::InvalidStructure(message))
                if message.contains("must be embedded")
        ));
        let mut wrong_parent = spec.clone();
        wrong_parent.message.message_class = "IPM.Note".to_owned();
        assert!(matches!(
            validate_spec(&wrong_parent),
            Err(WriterError::InvalidStructure(message))
                if message.contains("appointment parent")
        ));
        let mut missing_linkage = spec.clone();
        missing_linkage.message.attachments[1]
            .raw_properties
            .retain(|property| property.id != 0x7FFA);
        assert!(matches!(
            validate_spec(&missing_linkage),
            Err(WriterError::InvalidStructure(message))
                if message.contains("linkage properties")
        ));

        spec.message.attachments[1]
            .raw_properties
            .push(RawProperty {
                id: 0x370A,
                value: RawPropertyValue::Integer32(1),
            });
        assert!(matches!(
            validate_spec(&spec),
            Err(WriterError::InvalidStructure(message))
                if message.contains("not a supported calendar-exception property")
        ));
        spec.message.attachments[1].raw_properties.pop();
        spec.message.attachments[1].raw_properties[4].value = RawPropertyValue::Boolean(true);
        assert!(matches!(
            validate_spec(&spec),
            Err(WriterError::InvalidStructure(message))
                if message.contains("wrong calendar-exception type")
        ));
        Ok(())
    }

    #[test]
    fn every_supported_raw_value_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let mut spec = FidelityStore::default();
        spec.message.raw_properties = vec![
            RawProperty {
                id: 0x1100,
                value: RawPropertyValue::Integer16(-2),
            },
            RawProperty {
                id: 0x1101,
                value: RawPropertyValue::Integer32(-3),
            },
            RawProperty {
                id: 0x1102,
                value: RawPropertyValue::Integer64(-4),
            },
            RawProperty {
                id: 0x1103,
                value: RawPropertyValue::Floating32(1.25_f32.to_bits()),
            },
            RawProperty {
                id: 0x1104,
                value: RawPropertyValue::Floating64((-2.5_f64).to_bits()),
            },
            RawProperty {
                id: 0x1105,
                value: RawPropertyValue::Currency(123_456),
            },
            RawProperty {
                id: 0x1106,
                value: RawPropertyValue::FloatingTime(45_000.5_f64.to_bits()),
            },
            RawProperty {
                id: 0x1107,
                value: RawPropertyValue::ErrorCode(0x8000_4005),
            },
            RawProperty {
                id: 0x1108,
                value: RawPropertyValue::Boolean(true),
            },
            RawProperty {
                id: 0x1109,
                value: RawPropertyValue::Time(133_801_632_000_000_000),
            },
            RawProperty {
                id: 0x110A,
                value: RawPropertyValue::Guid(*b"PSTForgeRawValue"),
            },
            RawProperty {
                id: 0x110B,
                value: RawPropertyValue::Unicode("raw Unicode".to_owned()),
            },
            RawProperty {
                id: 0x110C,
                value: RawPropertyValue::Binary(vec![0, 1, 2, 255]),
            },
            RawProperty {
                id: 0x110D,
                value: RawPropertyValue::MultipleInteger16(vec![-1, 2]),
            },
            RawProperty {
                id: 0x110E,
                value: RawPropertyValue::MultipleInteger32(vec![-3, 4]),
            },
            RawProperty {
                id: 0x110F,
                value: RawPropertyValue::MultipleInteger64(vec![-5, 6]),
            },
            RawProperty {
                id: 0x1110,
                value: RawPropertyValue::MultipleGuid(vec![*b"PSTForgeGuidOne!"]),
            },
            RawProperty {
                id: 0x1111,
                value: RawPropertyValue::MultipleGuid(Vec::new()),
            },
        ];
        let directory = tempfile::tempdir()?;
        create_fidelity_store(directory.path().join("raw-values.pst"), &spec)?;
        Ok(())
    }

    #[test]
    fn optional_absence_and_embedded_rtf_sync_round_trip() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut spec = FidelityStore::default();
        spec.message.body_text = None;
        spec.message.body_html = None;
        spec.message.body_rtf = None;
        spec.message.native_body = None;
        spec.message.internet_headers = None;
        if let AttachmentContent::Embedded(embedded) = &mut spec.message.attachments[1].content {
            embedded.body_rtf = Some(br"{\rtf1\ansi synchronized embedded body}".to_vec());
            embedded.native_body = None;
            embedded.rtf_in_sync = true;
            embedded.internet_headers =
                Some("Message-ID: <embedded-checkpoint@example.com>\r\n".to_owned());
        }
        let directory = tempfile::tempdir()?;
        create_fidelity_store(directory.path().join("optional-values.pst"), &spec)?;
        Ok(())
    }
}
