//! Creation of compact Unicode version 23 PST stores.

use byteorder::{LittleEndian, WriteBytesExt};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read, Seek, SeekFrom, Write},
    ops::Range,
    os::fd::AsRawFd,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

use crate::{
    block_sig::compute_sig,
    ltp::{
        heap::{HeapFillLevel, HeapId, HeapNodeHeader, HeapNodeType},
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
            UnicodeDataTreeEntry, UnicodeLeafSubNodeTreeBlock, UnicodeLeafSubNodeTreeEntry,
            UnicodeSubNodeTreeBlockHeader, block_size,
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

const FILE_EOF: u64 = 0x42400;
const FIRST_AMAP: u64 = 0x4400;
const FIRST_PMAP: u64 = 0x4600;
const FIRST_DATA: u64 = 0x4800;
const SLOTS_PER_AMAP: u64 = 496 * 8;
const SLOT_SIZE: u64 = 64;
const PAGE_SIZE: u64 = 512;
const IPM_FOLDER_INDEX: u32 = 0x401;
const SEARCH_ROOT_INDEX: u32 = 0x402;
const DELETED_FOLDER_INDEX: u32 = 0x403;
const MAIL_FOLDER_INDEX: u32 = 0x404;
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
    Embedded(Box<MessageSpec>),
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
    pub subject: String,
    pub sender_name: String,
    pub sender_email: String,
    pub recipients: Vec<RecipientSpec>,
    pub sent_filetime: i64,
    pub received_filetime: i64,
    pub body_text: Option<String>,
    pub body_html: Option<Vec<u8>>,
    pub body_rtf: Option<Vec<u8>>,
    pub native_body: Option<NativeBody>,
    pub rtf_in_sync: bool,
    pub internet_headers: Option<String>,
    pub attachments: Vec<AttachmentSpec>,
    pub named_properties: Vec<NamedProperty>,
    pub raw_properties: Vec<RawProperty>,
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

impl From<&MinimalStore> for FidelityStore {
    fn from(spec: &MinimalStore) -> Self {
        const FIXED_FILETIME: i64 = 133_801_632_000_000_000;
        Self {
            store_name: spec.store_name.clone(),
            folder_name: spec.folder_name.clone(),
            record_key: spec.record_key,
            message: MessageSpec {
                message_class: "IPM.Note".to_owned(),
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
                body_text: Some(spec.body.clone()),
                body_html: None,
                body_rtf: None,
                native_body: Some(NativeBody::PlainText),
                rtf_in_sync: false,
                internet_headers: None,
                attachments: Vec::new(),
                named_properties: Vec::new(),
                raw_properties: Vec::new(),
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
            unsupported_properties: Vec::new(),
        };
        Self {
            store_name: "PSTForge 0.2.1 Mail Fidelity".to_owned(),
            folder_name: "Fidelity Mail".to_owned(),
            record_key: *b"PSTFORGE-0.2.1!!",
            message: MessageSpec {
                message_class: "IPM.Note".to_owned(),
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
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
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
    physical_size: u64,
    ref_count: u16,
}

struct TableRowSpec {
    id: NodeId,
    values: Vec<(u16, PropertyValue)>,
}

const MAX_DATA_BLOCK_PAYLOAD: usize = 8176;
const MAX_DATA_TREE_ENTRIES: usize = 1021;
const MAX_FIDELITY_PROPERTY_BYTES: usize = 16 * 1024;
const MAX_FIDELITY_COLLECTION_ITEMS: usize = MAX_FIDELITY_PROPERTY_BYTES / 8;
const MAX_FIDELITY_CUSTOM_PROPERTY_BYTES: usize = 64 * 1024;

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
    let mut leaves = Vec::with_capacity(bytes.len().div_ceil(MAX_DATA_BLOCK_PAYLOAD));
    for chunk in bytes.chunks(MAX_DATA_BLOCK_PAYLOAD) {
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

/// Create a deterministic Unicode PST containing one canonical mail message.
pub fn create_fidelity_store(
    path: impl AsRef<Path>,
    spec: &FidelityStore,
) -> Result<FidelityWriteReport, WriterError> {
    validate_spec(spec)?;
    let report = FidelityWriteReport {
        unsupported_properties: collect_unsupported_properties(&spec.message, &[])?,
    };
    let path = path.as_ref();
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
    file.set_len(FILE_EOF)?;

    let root_folder = NID_ROOT_FOLDER;
    let ipm_folder = node(NodeIdType::NormalFolder, IPM_FOLDER_INDEX)?;
    let search_root = node(NodeIdType::NormalFolder, SEARCH_ROOT_INDEX)?;
    let deleted_folder = node(NodeIdType::NormalFolder, DELETED_FOLDER_INDEX)?;
    let mail_folder = node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX)?;
    let spam_search = node(NodeIdType::SearchFolder, SPAM_SEARCH_INDEX)?;
    let message = node(NodeIdType::NormalMessage, MESSAGE_INDEX)?;

    let hierarchy_columns = hierarchy_columns()?;
    let contents_columns = contents_columns()?;
    let associated_columns = associated_columns()?;
    let search_contents_columns = search_contents_columns()?;
    let receive_folder_columns = receive_folder_columns()?;
    let outgoing_queue_columns = outgoing_queue_columns()?;
    let contents_index_columns = contents_index_columns()?;
    let search_index_columns = search_index_columns()?;
    let attachment_index_columns = attachment_index_columns()?;
    let recipient_columns = recipient_columns()?;
    let attachment_columns = attachment_columns()?;
    let named_identities = collect_named_identities(&spec.message);
    let recipient_rows = spec
        .message
        .recipients
        .iter()
        .enumerate()
        .map(|(index, recipient)| recipient_table_row(index, recipient))
        .collect::<Result<Vec<_>, _>>()?;
    let recipient_table = table_context(&recipient_columns, &recipient_rows)?;
    let mut attachment_rows = Vec::new();
    let mut attachment_blocks = Vec::new();
    let mut message_subnodes = vec![
        UnicodeLeafSubNodeTreeEntry::new(
            NodeId::from(NID_RECIPIENT_TABLE_TEMPLATE),
            leaf_bid(17)?,
            None,
        ),
        UnicodeLeafSubNodeTreeEntry::new(
            NodeId::from(NID_ATTACHMENT_TABLE_TEMPLATE),
            leaf_bid(18)?,
            None,
        ),
    ];
    let mut next_block_index = 28_u64;
    let mut next_value_node = 0x4_0000_u32;
    for (index, attachment) in spec.message.attachments.iter().enumerate() {
        let attachment_index =
            u32::try_from(index).map_err(|_| WriterError::ValueTooLarge("attachment count"))?;
        let attachment_node = node(
            NodeIdType::Attachment,
            0x2_0000_u32
                .checked_add(attachment_index)
                .ok_or(WriterError::ValueTooLarge("attachment node"))?,
        )?;
        let attachment_block = take_block_id(&mut next_block_index, false)?;
        let mut attachment_local_subnodes = Vec::new();
        let (method, data_property) = match &attachment.content {
            AttachmentContent::Binary(data) => (1, PropertyValue::Binary(data.clone())),
            AttachmentContent::Embedded(embedded) => {
                let embedded_node = node(
                    NodeIdType::NormalMessage,
                    0x3_0000_u32
                        .checked_add(attachment_index)
                        .ok_or(WriterError::ValueTooLarge("embedded message node"))?,
                )?;
                let embedded_recipient_rows = embedded
                    .recipients
                    .iter()
                    .enumerate()
                    .map(|(row, recipient)| recipient_table_row(row, recipient))
                    .collect::<Result<Vec<_>, _>>()?;
                let embedded_recipients =
                    table_context(&recipient_columns, &embedded_recipient_rows)?;
                let embedded_attachments = table_context(&attachment_columns, &[])?;
                let embedded_blocks_start = attachment_blocks.len();
                // Keep the embedded message PC before its table BIDs. Outlook and
                // libpff both assume this canonical allocation order while walking
                // attachment-local descriptors.
                let embedded_pc_block = take_block_id(&mut next_block_index, false)?;
                let embedded_recipient_block = take_block_id(&mut next_block_index, false)?;
                let embedded_attachment_block = take_block_id(&mut next_block_index, false)?;
                let mut embedded_subnodes = vec![
                    UnicodeLeafSubNodeTreeEntry::new(
                        NodeId::from(NID_RECIPIENT_TABLE_TEMPLATE),
                        embedded_recipient_block,
                        None,
                    ),
                    UnicodeLeafSubNodeTreeEntry::new(
                        NodeId::from(NID_ATTACHMENT_TABLE_TEMPLATE),
                        embedded_attachment_block,
                        None,
                    ),
                ];
                let embedded_key = message_record_key(spec.record_key, embedded_node);
                let mut embedded_properties =
                    message_properties(embedded, &named_identities, embedded_key, 0)?;
                externalize_large_properties(
                    &mut embedded_properties,
                    &mut next_block_index,
                    &mut next_value_node,
                    &mut attachment_blocks,
                    &mut embedded_subnodes,
                )?;
                let embedded_pc_zero = property_context(&embedded_properties)?;
                let embedded_bytes = embedded_pc_zero
                    .len()
                    .checked_add(embedded_recipients.len())
                    .and_then(|total| total.checked_add(embedded_attachments.len()))
                    .and_then(|total| {
                        attachment_blocks[embedded_blocks_start..]
                            .iter()
                            .try_fold(total, |sum, block| {
                                sum.checked_add(block.payload.logical_size())
                            })
                    })
                    .ok_or(WriterError::ValueTooLarge("embedded message size"))?;
                let embedded_size = i32::try_from(embedded_bytes)
                    .map_err(|_| WriterError::ValueTooLarge("embedded message size"))?;
                set_message_size(&mut embedded_properties, embedded_size)?;
                let embedded_pc = property_context(&embedded_properties)?;
                let embedded_object_size = u32::try_from(embedded_size)
                    .map_err(|_| WriterError::ValueTooLarge("embedded message"))?;
                let embedded_subnode_block = take_block_id(&mut next_block_index, true)?;
                embedded_subnodes.sort_by_key(|entry| u32::from(entry.node()));
                attachment_blocks.extend([
                    BlockSpec {
                        id: embedded_pc_block,
                        payload: BlockPayload::Data(embedded_pc),
                        ref_count: 2,
                    },
                    BlockSpec {
                        id: embedded_recipient_block,
                        payload: BlockPayload::Data(embedded_recipients),
                        ref_count: 2,
                    },
                    BlockSpec {
                        id: embedded_attachment_block,
                        payload: BlockPayload::Data(embedded_attachments),
                        ref_count: 2,
                    },
                    BlockSpec {
                        id: embedded_subnode_block,
                        payload: BlockPayload::Subnode(embedded_subnodes),
                        ref_count: 2,
                    },
                ]);
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
        let size = attachment_property_size(&properties)?;
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
            &mut next_block_index,
            &mut next_value_node,
            &mut attachment_blocks,
            &mut attachment_local_subnodes,
        )?;
        attachment_blocks.push(BlockSpec {
            id: attachment_block,
            payload: BlockPayload::Data(property_context(&properties)?),
            ref_count: 2,
        });
        attachment_local_subnodes.sort_by_key(|entry| u32::from(entry.node()));
        let attachment_subnode = if attachment_local_subnodes.is_empty() {
            None
        } else {
            let block = take_block_id(&mut next_block_index, true)?;
            attachment_blocks.push(BlockSpec {
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
    let top_key = message_record_key(spec.record_key, message);
    let mut top_properties = message_properties(&spec.message, &named_identities, top_key, 0)?;
    externalize_large_properties(
        &mut top_properties,
        &mut next_block_index,
        &mut next_value_node,
        &mut attachment_blocks,
        &mut message_subnodes,
    )?;
    message_subnodes.sort_by_key(|entry| u32::from(entry.node()));
    let top_pc_zero = property_context(&top_properties)?;
    let message_bytes = top_pc_zero
        .len()
        .checked_add(recipient_table.len())
        .and_then(|total| total.checked_add(attachment_table.len()))
        .and_then(|total| {
            attachment_blocks.iter().try_fold(total, |sum, block| {
                sum.checked_add(block.payload.logical_size())
            })
        })
        .ok_or(WriterError::ValueTooLarge("message size"))?;
    let message_size =
        i32::try_from(message_bytes).map_err(|_| WriterError::ValueTooLarge("message size"))?;
    set_message_size(&mut top_properties, message_size)?;
    let top_property_context = property_context(&top_properties)?;
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
            payload: BlockPayload::Data(table_context(
                &hierarchy_columns,
                &[
                    folder_table_row(ipm_folder, "Top of Personal Folders", 0, true),
                    folder_table_row(search_root, "Search Root", 0, false),
                    folder_table_row(spam_search, "SPAM Search Folder 2", 0, false),
                ],
            )?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(5)?,
            payload: BlockPayload::Data(table_context(&contents_columns, &[])?),
            ref_count: 6,
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
            payload: BlockPayload::Data(table_context(
                &hierarchy_columns,
                &[
                    folder_table_row(deleted_folder, "Deleted Items", 0, false),
                    folder_table_row(mail_folder, &spec.folder_name, 1, false),
                ],
            )?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(8)?,
            payload: BlockPayload::Data(property_context(&folder_properties(
                "Deleted Items",
                0,
                false,
            ))?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(9)?,
            payload: BlockPayload::Data(table_context(&hierarchy_columns, &[])?),
            ref_count: 5,
        },
        BlockSpec {
            id: leaf_bid(10)?,
            payload: BlockPayload::Data(property_context(&folder_properties(
                &spec.folder_name,
                1,
                false,
            ))?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(11)?,
            payload: BlockPayload::Data(table_context(
                &contents_columns,
                &[message_table_row(message, spec, top_key, message_size)],
            )?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(12)?,
            payload: BlockPayload::Data(top_property_context),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(13)?,
            payload: BlockPayload::Data(table_context(&associated_columns, &[])?),
            ref_count: 7,
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
            id: leaf_bid(17)?,
            payload: BlockPayload::Data(recipient_table),
            ref_count: 3,
        },
        BlockSpec {
            id: leaf_bid(18)?,
            payload: BlockPayload::Data(attachment_table),
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
        BlockSpec {
            id: internal_bid(27)?,
            payload: BlockPayload::Subnode(message_subnodes),
            ref_count: 2,
        },
    ];
    blocks.extend(attachment_blocks);
    blocks.sort_by_key(|block| u64::from(block.id));

    let written = write_blocks(&mut *file, &blocks)?;
    let page_offset = align_up(
        written
            .last()
            .map(|block| block.offset + block.physical_size)
            .unwrap_or(FIRST_DATA),
        PAGE_SIZE,
    );
    let (bbt, nbt_offset, next_page_id) = write_bbt(&mut *file, page_offset, 0x100, &written)?;
    let nodes = node_entries(
        root_folder,
        ipm_folder,
        search_root,
        deleted_folder,
        mail_folder,
        spam_search,
        message,
    )?;
    let (nbt, allocated_end, next_page_id) =
        write_nbt(&mut *file, nbt_offset, next_page_id, &nodes)?;

    write_fixed_pages(&mut *file, allocated_end, UnicodePageId::from(next_page_id))?;
    write_header(
        &mut *file,
        nbt,
        bbt,
        allocated_end,
        UnicodePageId::from(next_page_id),
        leaf_bid(next_block_index)?,
        nid_counters(&nodes, &blocks)?,
    )?;
    file.sync_all()?;
    let validated_path = PathBuf::from(format!(
        "/proc/{}/fd/{}",
        std::process::id(),
        temporary.file.as_raw_fd()
    ));
    validate_completed_store(&validated_path, spec)?;
    validate_with_independent_readers(&validated_path, &mut temporary)?;
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

fn run_validator(command: &mut Command, timeout: Duration) -> io::Result<ValidatorOutput> {
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
    let timed_out = loop {
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
            break false;
        }
        if Instant::now() >= deadline {
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
            break true;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let status = status.ok_or_else(|| io::Error::other("validator status is unavailable"))?;
    let (stdout, stdout_truncated) = stdout_result
        .ok_or_else(|| io::Error::other("validator stdout result is unavailable"))??;
    let (stderr, stderr_truncated) = stderr_result
        .ok_or_else(|| io::Error::other("validator stderr result is unavailable"))??;
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
) -> Result<(), WriterError> {
    let mut pffinfo = Command::new("pffinfo");
    pffinfo.arg(path);
    let outcome = run_validator(&mut pffinfo, VALIDATOR_TIMEOUT).map_err(|source| {
        WriterError::IndependentValidatorIo {
            tool: "pffinfo",
            source,
        }
    })?;
    if !outcome.success {
        let evidence = temporary.retain_validation_failure("pffinfo", &outcome)?;
        return Err(WriterError::IndependentValidation {
            tool: "pffinfo",
            evidence,
        });
    }

    let output = tempfile::tempdir().map_err(|source| WriterError::IndependentValidatorIo {
        tool: "readpst scratch directory",
        source,
    })?;
    let mut readpst = Command::new("readpst");
    readpst
        .args(["-q", "-r", "-o"])
        .arg(output.path())
        .arg(path);
    let outcome = run_validator(&mut readpst, VALIDATOR_TIMEOUT).map_err(|source| {
        WriterError::IndependentValidatorIo {
            tool: "readpst",
            source,
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

struct PublicationTemporary {
    file: std::fs::File,
    source_name: std::ffi::OsString,
    directory: std::fs::File,
    retain: bool,
}

impl PublicationTemporary {
    fn new(parent: &Path) -> Result<Self, WriterError> {
        let directory = tempfile::Builder::new()
            .prefix(".pstforge-")
            .tempdir_in(parent)?;
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
            retain: false,
        })
    }

    fn source_name(&self) -> &std::ffi::OsStr {
        &self.source_name
    }

    fn directory_path(&self) -> io::Result<PathBuf> {
        std::fs::read_link(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
    }

    fn retain_validation_failure(
        &mut self,
        tool: &'static str,
        outcome: &ValidatorOutput,
    ) -> Result<PathBuf, WriterError> {
        self.retain = true;
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
            format!("stderr truncated: {}\nstdout:\n", outcome.stderr_truncated).as_bytes(),
        );
        diagnostic.extend_from_slice(&outcome.stdout);
        diagnostic.extend_from_slice(b"\nstderr:\n");
        diagnostic.extend_from_slice(&outcome.stderr);
        let diagnostic_file = std::fs::File::create(diagnostic_path)?;
        (&diagnostic_file).write_all(&diagnostic)?;
        diagnostic_file.sync_all()?;
        self.directory.sync_all()?;
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
    validate_message(&spec.message, false)
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
    match &attachment.content {
        AttachmentContent::Binary(data) if data.len() <= 2048 => lengths.push(data.len()),
        AttachmentContent::Embedded(_) => lengths.push(8),
        AttachmentContent::Binary(_) => {}
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
    let mut property_count = 18_usize;
    let mut lengths = vec![
        unicode_payload_len(&message.message_class)?,
        unicode_payload_len(&message.subject)?,
        unicode_payload_len(&message.sender_name)?,
        8,
        unicode_payload_len(&message.sender_email)?,
        unicode_payload_len(&message.sender_name)?,
        8,
        unicode_payload_len(&message.sender_email)?,
        16,
        8,
        8,
        8,
        8,
    ];
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

fn validate_message(message: &MessageSpec, embedded: bool) -> Result<(), WriterError> {
    if message.message_class != "IPM.Note" && !message.message_class.starts_with("IPM.Note.") {
        return Err(WriterError::InvalidStructure(format!(
            "unsupported message class: {}",
            message.message_class
        )));
    }
    for (name, value) in [
        ("message class", &message.message_class),
        ("subject", &message.subject),
        ("sender name", &message.sender_name),
        ("sender email", &message.sender_email),
    ] {
        if value.is_empty() {
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
    if message.body_rtf.is_none() && message.rtf_in_sync {
        return Err(WriterError::InvalidStructure(
            "RTF cannot be marked synchronized when no RTF body is present".to_owned(),
        ));
    }
    match message.native_body {
        Some(NativeBody::PlainText) if message.body_text.is_none() => {
            return Err(WriterError::InvalidStructure(
                "native plain-text body is not present".to_owned(),
            ));
        }
        Some(NativeBody::Rtf) if message.body_rtf.is_none() => {
            return Err(WriterError::InvalidStructure(
                "native RTF body is not present".to_owned(),
            ));
        }
        Some(NativeBody::Html) if message.body_html.is_none() => {
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
        if let AttachmentContent::Binary(data) = &attachment.content {
            validate_payload_len("attachment payload", data.len())?;
        }
        if let AttachmentContent::Embedded(child) = &attachment.content {
            if embedded || !child.attachments.is_empty() {
                return Err(WriterError::InvalidStructure(
                    "nested attachments inside an embedded message are not supported".to_owned(),
                ));
            }
            validate_message(child, true)?;
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
    validate_message_property_context_shape(message)?;
    Ok(())
}

fn validate_unicode(name: &'static str, value: &str) -> Result<(), WriterError> {
    if value.encode_utf16().count() > 2048 {
        return Err(WriterError::ValueTooLarge(name));
    }
    Ok(())
}

fn validate_raw_value(value: &RawPropertyValue) -> Result<(), WriterError> {
    if matches!(
        value,
        RawPropertyValue::Unicode(value) if value.is_empty()
    ) || matches!(value, RawPropertyValue::Binary(value) if value.is_empty())
        || matches!(value, RawPropertyValue::MultipleInteger16(value) if value.is_empty())
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

fn validate_completed_store(path: &Path, spec: &FidelityStore) -> Result<(), WriterError> {
    use crate::{ltp::prop_context::PropertyValue as ReadValue, messaging::store::EntryId};

    let invalid = |message: &str| WriterError::InvalidStructure(message.to_owned());
    let store = crate::open_store(path)?;
    if store.properties().display_name()? != spec.store_name {
        return Err(invalid("completed store display name mismatch"));
    }

    let ipm = node(NodeIdType::NormalFolder, IPM_FOLDER_INDEX)?;
    let mail = node(NodeIdType::NormalFolder, MAIL_FOLDER_INDEX)?;
    let message = node(NodeIdType::NormalMessage, MESSAGE_INDEX)?;
    let ipm_entry = store.properties().make_entry_id(ipm)?;
    let ipm_folder = store.open_folder(&ipm_entry)?;
    let hierarchy = ipm_folder
        .hierarchy_table()
        .ok_or_else(|| invalid("completed store IPM hierarchy table is missing"))?;
    hierarchy
        .find_row(crate::ltp::table_context::TableRowId::new(u32::from(mail)))
        .map_err(|_| invalid("completed store mail folder is not indexed"))?;
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
    if !matches!(table_value(0x0039)?, Some(ReadValue::Time(value)) if value == spec.message.sent_filetime)
        || !matches!(table_value(0x0042)?, Some(ReadValue::Unicode(value)) if value.to_string() == spec.message.sender_name)
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

    let message_entry = EntryId::new(
        crate::messaging::store::StoreRecordKey::new(spec.record_key),
        message,
    );
    let message = store.open_message(&message_entry, None)?;
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
    let expected_flags = if spec.message.attachments.is_empty() {
        1
    } else {
        0x11
    };
    if !matches!(message.properties().get(0x0E07), Some(ReadValue::Integer32(value)) if *value == expected_flags)
        || !matches!(message.properties().get(0x0E1B), Some(ReadValue::Boolean(value)) if *value != spec.message.attachments.is_empty())
        || !matches!(
            message.properties().get(0x3FDE),
            Some(ReadValue::Integer32(65001))
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
    match (&spec.message.body_rtf, message.properties().get(0x0E1F)) {
        (Some(_), Some(ReadValue::Boolean(actual))) if *actual == spec.message.rtf_in_sync => {}
        (None, None) => {}
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
        message.properties().get(0x3008),
        Some(ReadValue::Time(value)) if *value == spec.message.received_filetime
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
    let named_identities = collect_named_identities(&spec.message);
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
            return Err(invalid("completed store named property mismatch"));
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
    validate_attachment_fidelity(path, spec, &named_identities)?;
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
    spec: &FidelityStore,
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
        None,
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
        let attachment =
            UnicodeAttachment::read(top.clone(), attachment_node, None).map_err(|error| {
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
                    actual.as_ref(),
                    expected_message,
                    named_identities,
                    message_record_key(spec.record_key, embedded_node),
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
    actual: &dyn crate::messaging::message::Message,
    expected: &MessageSpec,
    named_identities: &[NamedIdentity],
    record_key: [u8; 16],
) -> Result<(), WriterError> {
    use crate::ltp::prop_context::PropertyValue as ReadValue;

    let invalid = |message: &str| WriterError::InvalidStructure(message.to_owned());
    let properties = actual.properties();
    let unicode_matches = |id, expected: &str| matches!(properties.get(id), Some(ReadValue::Unicode(value)) if value.to_string() == expected);
    if properties.message_class()? != expected.message_class
        || !unicode_matches(0x0037, &expected.subject)
        || !unicode_matches(0x0042, &expected.sender_name)
        || !unicode_matches(0x0065, &expected.sender_email)
        || !unicode_matches(0x0C1A, &expected.sender_name)
        || !unicode_matches(0x0C1F, &expected.sender_email)
        || !unicode_matches(0x0064, "SMTP")
        || !unicode_matches(0x0C1E, "SMTP")
        || !matches!(properties.get(0x0039), Some(ReadValue::Time(value)) if *value == expected.sent_filetime)
        || !matches!(properties.get(0x0E06), Some(ReadValue::Time(value)) if *value == expected.received_filetime)
        || !matches!(properties.get(0x3008), Some(ReadValue::Time(value)) if *value == expected.received_filetime)
        || !matches!(properties.get(0x300B), Some(ReadValue::Binary(value)) if value.buffer() == record_key)
        || !matches!(properties.get(0x0E07), Some(ReadValue::Integer32(value)) if *value == if expected.attachments.is_empty() { 1 } else { 0x11 })
        || !matches!(properties.get(0x0E1B), Some(ReadValue::Boolean(value)) if *value != expected.attachments.is_empty())
        || !matches!(properties.get(0x3FDE), Some(ReadValue::Integer32(65001)))
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
    match (&expected.body_rtf, properties.get(0x0E1F)) {
        (Some(_), Some(ReadValue::Boolean(actual))) if *actual == expected.rtf_in_sync => {}
        (None, None) => {}
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
    Ok(())
}

fn store_properties(
    spec: &FidelityStore,
    ipm_folder: NodeId,
    deleted_folder: NodeId,
    search_root: NodeId,
) -> Result<Vec<(u16, PropertyValue)>, WriterError> {
    Ok(vec![
        (0x0E34, PropertyValue::Binary(spec.record_key.to_vec())),
        (0x0FF9, PropertyValue::Binary(spec.record_key.to_vec())),
        (0x3001, PropertyValue::Unicode(spec.store_name.clone())),
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
    vec![
        (0x3001, PropertyValue::Unicode(name.to_owned())),
        (0x3601, PropertyValue::Integer32(1)),
        (0x3602, PropertyValue::Integer32(content_count)),
        (0x3603, PropertyValue::Integer32(0)),
        (0x360A, PropertyValue::Boolean(has_children)),
        (0x3613, PropertyValue::Unicode("IPF.Note".to_owned())),
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
        (0x0042, PropertyValue::Unicode(message.sender_name.clone())),
        (0x0064, PropertyValue::Unicode("SMTP".to_owned())),
        (0x0065, PropertyValue::Unicode(message.sender_email.clone())),
        (0x0C1A, PropertyValue::Unicode(message.sender_name.clone())),
        (0x0C1E, PropertyValue::Unicode("SMTP".to_owned())),
        (0x0C1F, PropertyValue::Unicode(message.sender_email.clone())),
        (0x0E06, PropertyValue::Time(message.received_filetime)),
        (
            0x0E07,
            PropertyValue::Integer32(if message.attachments.is_empty() {
                1
            } else {
                0x11
            }),
        ),
        (0x0E08, PropertyValue::Integer32(message_size)),
        (0x0E17, PropertyValue::Integer32(0)),
        (
            0x0E1B,
            PropertyValue::Boolean(!message.attachments.is_empty()),
        ),
        (0x3007, PropertyValue::Time(message.received_filetime)),
        (0x3008, PropertyValue::Time(message.received_filetime)),
        (0x300B, PropertyValue::Binary(record_key.to_vec())),
        (0x3FDE, PropertyValue::Integer32(65001)),
    ];
    if let Some(body) = &message.body_text {
        properties.push((0x1000, PropertyValue::Unicode(body.clone())));
    }
    if let Some(html) = &message.body_html {
        properties.push((0x1013, PropertyValue::Binary(html.clone())));
    }
    if let Some(rtf) = &message.body_rtf {
        properties.push((0x1009, PropertyValue::Binary(rtf_container(rtf)?)));
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
    properties
}

fn attachment_property_size(properties: &[(u16, PropertyValue)]) -> Result<i32, WriterError> {
    let size = properties.iter().try_fold(0_usize, |total, (_, value)| {
        let value_size = match value {
            PropertyValue::Integer32(_) => 4,
            PropertyValue::Unicode(value) => unicode_payload_len(value)?,
            PropertyValue::Binary(value) => value.len(),
            PropertyValue::Object(_, size) => usize::try_from(*size)
                .map_err(|_| WriterError::ValueTooLarge("attachment object"))?,
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
    TableRowSpec {
        id,
        values: vec![
            (0x3001, PropertyValue::Unicode(name.to_owned())),
            (0x3602, PropertyValue::Integer32(count)),
            (0x3603, PropertyValue::Integer32(0)),
            (0x360A, PropertyValue::Boolean(children)),
            (0x3613, PropertyValue::Unicode("IPF.Note".to_owned())),
        ],
    }
}

fn message_table_row(
    id: NodeId,
    spec: &FidelityStore,
    record_key: [u8; 16],
    message_size: i32,
) -> TableRowSpec {
    let message = &spec.message;
    let mut values = vec![
        (
            0x001A,
            PropertyValue::Unicode(message.message_class.clone()),
        ),
        (0x0037, PropertyValue::Unicode(message.subject.clone())),
        (0x0039, PropertyValue::Time(message.sent_filetime)),
        (0x0042, PropertyValue::Unicode(message.sender_name.clone())),
        (0x0E06, PropertyValue::Time(message.received_filetime)),
        (
            0x0E07,
            PropertyValue::Integer32(if message.attachments.is_empty() {
                1
            } else {
                0x11
            }),
        ),
        (0x0E08, PropertyValue::Integer32(message_size)),
        (0x0E17, PropertyValue::Integer32(0)),
        (0x0E30, PropertyValue::Binary(record_key.to_vec())),
        (0x0E33, PropertyValue::Integer64(0x90)),
        (
            0x0E34,
            PropertyValue::Binary(message_instance_entry_id(spec.record_key)),
        ),
        (0x3008, PropertyValue::Time(message.received_filetime)),
    ];
    values.extend(
        display_recipient_properties(&message.recipients)
            .into_iter()
            .filter(|(id, _)| matches!(*id, 0x0E03 | 0x0E04)),
    );
    TableRowSpec { id, values }
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

fn write_blocks(
    file: &mut std::fs::File,
    blocks: &[BlockSpec],
) -> Result<Vec<WrittenBlock>, WriterError> {
    let mut offset = FIRST_DATA;
    let mut written = Vec::with_capacity(blocks.len());
    for block in blocks {
        file.seek(SeekFrom::Start(offset))?;
        let size = u16::try_from(block.payload.logical_size())
            .map_err(|_| WriterError::ValueTooLarge("data block"))?;
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
        }
        let physical_size = u64::from(block_size(size.saturating_add(16))?);
        written.push(WrittenBlock {
            id: block.id,
            offset,
            size,
            physical_size,
            ref_count: block.ref_count,
        });
        offset = offset
            .checked_add(physical_size)
            .ok_or(WriterError::ValueTooLarge("file offset"))?;
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
    for (index, range) in pages.iter().enumerate() {
        let index = u64::try_from(index).map_err(|_| WriterError::ValueTooLarge("BBT page"))?;
        let offset = first_offset
            .checked_add(index.saturating_mul(PAGE_SIZE))
            .ok_or(WriterError::ValueTooLarge("BBT offset"))?;
        let page_id = UnicodePageId::from(first_page_id.saturating_add(index));
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
    }

    let page_count =
        u64::try_from(pages.len()).map_err(|_| WriterError::ValueTooLarge("BBT page count"))?;
    if roots.len() == 1 {
        return Ok((
            roots[0].block(),
            first_offset + PAGE_SIZE,
            first_page_id + 1,
        ));
    }
    let root_offset = first_offset
        .checked_add(page_count.saturating_mul(PAGE_SIZE))
        .ok_or(WriterError::ValueTooLarge("BBT root offset"))?;
    let root_page_id = UnicodePageId::from(first_page_id.saturating_add(page_count));
    let root = UnicodeBTreeEntryPage::new(
        1,
        20,
        24,
        &roots,
        page_trailer(PageType::BlockBTree, root_offset, root_page_id),
    )
    .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
    file.seek(SeekFrom::Start(root_offset))?;
    root.write(file)?;
    Ok((
        UnicodePageRef::new(root_page_id, UnicodeByteIndex::new(root_offset)),
        root_offset + PAGE_SIZE,
        first_page_id + page_count + 1,
    ))
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
    for (index, range) in pages.iter().enumerate() {
        let index = u64::try_from(index).map_err(|_| WriterError::ValueTooLarge("NBT page"))?;
        let offset = first_offset
            .checked_add(index.saturating_mul(PAGE_SIZE))
            .ok_or(WriterError::ValueTooLarge("NBT offset"))?;
        let page_id = UnicodePageId::from(first_page_id.saturating_add(index));
        let trailer = page_trailer(PageType::NodeBTree, offset, page_id);
        let page = UnicodeNodeBTreePage::new(0, 15, 32, &entries[range.clone()], trailer)
            .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
        file.seek(SeekFrom::Start(offset))?;
        page.write(file)?;
        roots.push(<UnicodeBTreePageEntry as BTreePageEntryReadWrite>::new(
            entries[range.start].key(),
            UnicodePageRef::new(page_id, UnicodeByteIndex::new(offset)),
        ));
    }

    let page_count =
        u64::try_from(pages.len()).map_err(|_| WriterError::ValueTooLarge("NBT page count"))?;
    if roots.len() == 1 {
        let root = roots[0].block();
        return Ok((root, first_offset + PAGE_SIZE, first_page_id + 1));
    }
    let root_offset = first_offset
        .checked_add(page_count.saturating_mul(PAGE_SIZE))
        .ok_or(WriterError::ValueTooLarge("NBT root offset"))?;
    let root_page_id = UnicodePageId::from(first_page_id.saturating_add(page_count));
    let root = UnicodeBTreeEntryPage::new(
        1,
        20,
        24,
        &roots,
        page_trailer(PageType::NodeBTree, root_offset, root_page_id),
    )
    .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
    file.seek(SeekFrom::Start(root_offset))?;
    root.write(file)?;
    Ok((
        UnicodePageRef::new(root_page_id, UnicodeByteIndex::new(root_offset)),
        root_offset + PAGE_SIZE,
        first_page_id + page_count + 1,
    ))
}

fn node_entries(
    root: NodeId,
    ipm: NodeId,
    search_root: NodeId,
    deleted: NodeId,
    mail: NodeId,
    spam_search: NodeId,
    message: NodeId,
) -> Result<Vec<UnicodeNodeBTreeEntry>, WriterError> {
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
        UnicodeNodeBTreeEntry::new(deleted, leaf_bid(8)?, None, Some(ipm)),
        table_node(deleted, NodeIdType::HierarchyTable, leaf_bid(9)?)?,
        table_node(deleted, NodeIdType::ContentsTable, leaf_bid(5)?)?,
        table_node(deleted, NodeIdType::AssociatedContentsTable, leaf_bid(13)?)?,
        UnicodeNodeBTreeEntry::new(mail, leaf_bid(10)?, None, Some(ipm)),
        table_node(mail, NodeIdType::HierarchyTable, leaf_bid(9)?)?,
        table_node(mail, NodeIdType::ContentsTable, leaf_bid(11)?)?,
        table_node(mail, NodeIdType::AssociatedContentsTable, leaf_bid(13)?)?,
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
        UnicodeNodeBTreeEntry::new(message, leaf_bid(12)?, Some(internal_bid(27)?), Some(mail)),
    ];
    entries.sort_by_key(|entry| entry.key());
    Ok(entries)
}

fn table_node(
    folder: NodeId,
    kind: NodeIdType,
    data: UnicodeBlockId,
) -> Result<UnicodeNodeBTreeEntry, WriterError> {
    Ok(UnicodeNodeBTreeEntry::new(
        node(kind, folder.index())?,
        data,
        None,
        None,
    ))
}

fn write_fixed_pages(
    file: &mut std::fs::File,
    allocated_end: u64,
    next_page_id: UnicodePageId,
) -> Result<(), WriterError> {
    if allocated_end > FILE_EOF {
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

fn write_header(
    file: &mut std::fs::File,
    nbt: UnicodePageRef,
    bbt: UnicodePageRef,
    allocated_end: u64,
    next_page_id: UnicodePageId,
    next_block_id: UnicodeBlockId,
    nids: [u32; 32],
) -> Result<(), WriterError> {
    let allocated_slots = allocated_end
        .checked_sub(FIRST_AMAP)
        .ok_or_else(|| WriterError::InvalidStructure("allocation start underflow".to_owned()))?
        .div_ceil(SLOT_SIZE);
    let free_slots = SLOTS_PER_AMAP
        .checked_sub(allocated_slots)
        .ok_or(WriterError::ValueTooLarge("allocation map"))?;
    let free_bytes = free_slots
        .checked_mul(SLOT_SIZE)
        .ok_or(WriterError::ValueTooLarge("free byte count"))?;
    let root = UnicodeRoot::new(
        UnicodeByteIndex::new(FILE_EOF),
        UnicodeByteIndex::new(FIRST_AMAP),
        UnicodeByteIndex::new(free_bytes),
        UnicodeByteIndex::new(0),
        nbt,
        bbt,
        AmapStatus::Valid2,
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
        let spec = FidelityStore::from(&minimal);
        let message = node(NodeIdType::NormalMessage, MESSAGE_INDEX)?;
        let recipients = table_context(&recipient_columns()?, &[])?;
        let attachments = table_context(&attachment_columns()?, &[])?;
        let record_key = message_record_key(spec.record_key, message);
        let mut properties = message_properties(&spec.message, &[], record_key, 0)?;
        let message_size = i32::try_from(
            property_context(&properties)?.len() + recipients.len() + attachments.len(),
        )?;
        set_message_size(&mut properties, message_size)?;
        let row = message_table_row(message, &spec, record_key, message_size);
        let contents_ids = contents_columns()?
            .into_iter()
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
        let no_recipient_row = message_table_row(message, &no_recipients, record_key, message_size);
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
    fn fidelity_store_round_trips_rich_mail() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("fidelity.pst");
        let spec = FidelityStore::default();
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
            AttachmentContent::Embedded(_) => return Err("expected binary attachment".into()),
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
        let output = run_validator(&mut command, Duration::from_millis(25))?;
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
        assert!(diagnostics.contains("bounded stderr"));
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
    fn fidelity_validation_rejects_ambiguous_inputs() -> Result<(), Box<dyn std::error::Error>> {
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
        assert!(matches!(
            validate_spec(&empty_raw),
            Err(WriterError::InvalidStructure(_))
        ));

        let mut invalid_html = FidelityStore::default();
        invalid_html.message.body_html = Some(vec![0xFF]);
        assert!(matches!(
            validate_spec(&invalid_html),
            Err(WriterError::InvalidStructure(_))
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
                content: AttachmentContent::Binary(vec![1]),
            });
        }
        let directory = tempfile::tempdir()?;
        assert!(matches!(
            create_fidelity_store(directory.path().join("nested.pst"), &nested),
            Err(WriterError::InvalidStructure(_))
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
            let properties = message_properties(&spec.message, &identities, [0; 16], 0)?;
            assert!(matches!(
                properties.iter().find(|(id, _)| *id == 0x1016),
                Some((_, PropertyValue::Integer32(actual))) if *actual == expected
            ));
        }
        spec.message.native_body = None;
        let properties = message_properties(&spec.message, &identities, [0; 16], 0)?;
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
            AttachmentContent::Binary(_) => return Err("expected embedded fixture".into()),
        }
        let identities = collect_named_identities(&spec.message);
        let embedded = match &spec.message.attachments[1].content {
            AttachmentContent::Embedded(message) => message,
            AttachmentContent::Binary(_) => return Err("expected embedded fixture".into()),
        };
        assert_eq!(identities.len(), 4);
        let properties = message_properties(
            embedded,
            &identities,
            message_record_key(spec.record_key, node(NodeIdType::NormalMessage, 0x3_0001)?),
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
