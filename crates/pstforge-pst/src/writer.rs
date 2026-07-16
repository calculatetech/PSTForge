//! Creation of compact Unicode version 23 PST stores.

use byteorder::{LittleEndian, WriteBytesExt};
use std::{
    io::{self, Seek, SeekFrom},
    ops::Range,
    path::{Path, PathBuf},
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
            UnicodeBlockTrailer, UnicodeDataBlock, UnicodeLeafSubNodeTreeBlock,
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
const MESSAGE_RECORD_KEY: [u8; 16] = [
    0x72, 0x3A, 0x41, 0x00, 0x72, 0xCB, 0xA5, 0x47, 0xB4, 0x3D, 0x82, 0xE7, 0x7C, 0xAC, 0xBF, 0xFA,
];
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
    #[error("writer value is too large for the PST structure: {0}")]
    ValueTooLarge(&'static str),
    #[error("invalid PST structure: {0}")]
    InvalidStructure(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Clone)]
enum PropertyValue {
    Integer32(i32),
    Integer64(i64),
    Boolean(bool),
    Time(i64),
    Unicode(String),
    Binary(Vec<u8>),
}

impl PropertyValue {
    fn property_type(&self) -> PropertyType {
        match self {
            Self::Integer32(_) => PropertyType::Integer32,
            Self::Integer64(_) => PropertyType::Integer64,
            Self::Boolean(_) => PropertyType::Boolean,
            Self::Time(_) => PropertyType::Time,
            Self::Unicode(_) => PropertyType::Unicode,
            Self::Binary(_) => PropertyType::Binary,
        }
    }

    fn inline_value(&self) -> Option<u32> {
        match self {
            Self::Integer32(value) => Some(u32::from_le_bytes(value.to_le_bytes())),
            Self::Boolean(value) => Some(u32::from(*value)),
            _ => None,
        }
    }

    fn variable_bytes(&self) -> io::Result<Option<Vec<u8>>> {
        let bytes = match self {
            Self::Integer64(value) | Self::Time(value) => value.to_le_bytes().to_vec(),
            Self::Unicode(value) => unicode_bytes(value)?,
            Self::Binary(value) => value.clone(),
            Self::Integer32(_) | Self::Boolean(_) => return Ok(None),
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
}

impl BlockPayload {
    fn logical_size(&self) -> usize {
        match self {
            Self::Data(data) => data.len(),
            Self::Subnode(entries) => 8_usize.saturating_add(entries.len().saturating_mul(24)),
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

/// Create a new PST with a minimal folder and plain-text message.
pub fn create_minimal_store(
    path: impl AsRef<Path>,
    spec: &MinimalStore,
) -> Result<(), WriterError> {
    validate_spec(spec)?;
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
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let file = temporary.as_file_mut();
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
    let recipient_table = table_context(&recipient_columns, &[])?;
    let message_size = serialized_message_size(spec, &recipient_table)?;
    let blocks = vec![
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
            payload: BlockPayload::Data(property_context(&named_property_map())?),
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
                &[message_table_row(message, spec, message_size)],
            )?),
            ref_count: 2,
        },
        BlockSpec {
            id: leaf_bid(12)?,
            payload: BlockPayload::Data(property_context(&message_properties(spec, message_size))?),
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
            payload: BlockPayload::Data(table_context(&attachment_columns, &[])?),
            ref_count: 2,
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
            payload: BlockPayload::Subnode(vec![UnicodeLeafSubNodeTreeEntry::new(
                NodeId::from(NID_RECIPIENT_TABLE_TEMPLATE),
                leaf_bid(17)?,
                None,
            )]),
            ref_count: 2,
        },
    ];

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
        leaf_bid(28)?,
        nid_counters(&nodes)?,
    )?;
    file.sync_all()?;
    validate_completed_store(temporary.path(), spec)?;
    publish_noclobber(&temporary, path)?;
    sync_published_directory(path, parent)?;
    Ok(())
}

fn sync_published_directory(destination: &Path, parent: &Path) -> Result<(), WriterError> {
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| WriterError::PublishedDurability {
            path: destination.to_path_buf(),
            source,
        })
}

fn publish_noclobber(
    temporary: &tempfile::NamedTempFile,
    destination: &Path,
) -> Result<(), WriterError> {
    use rustix::{
        fs::{CWD, RenameFlags, renameat_with},
        io::Errno,
    };

    match renameat_with(
        CWD,
        temporary.path(),
        CWD,
        destination,
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

fn validate_spec(spec: &MinimalStore) -> Result<(), WriterError> {
    for (name, value) in [
        ("store name", &spec.store_name),
        ("folder name", &spec.folder_name),
        ("subject", &spec.subject),
        ("body", &spec.body),
        ("sender name", &spec.sender_name),
        ("sender email", &spec.sender_email),
        ("recipient", &spec.recipient),
    ] {
        let units = value.encode_utf16().count();
        if units > 2048 {
            return Err(WriterError::ValueTooLarge(name));
        }
    }
    Ok(())
}

fn validate_completed_store(path: &Path, spec: &MinimalStore) -> Result<(), WriterError> {
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

    let message_entry = EntryId::new(
        crate::messaging::store::StoreRecordKey::new(spec.record_key),
        message,
    );
    let message = store.open_message(&message_entry, None)?;
    if message.properties().message_class()? != "IPM.Note" {
        return Err(invalid("completed store message class mismatch"));
    }
    if message.properties().message_size()? != row_size {
        return Err(invalid("completed store message-size values disagree"));
    }
    for (property, expected, name) in [
        (0x0037, &spec.subject, "subject"),
        (0x1000, &spec.body, "body"),
    ] {
        match message.properties().get(property) {
            Some(ReadValue::Unicode(value)) if value.to_string() == *expected => {}
            _ => return Err(invalid(&format!("completed store {name} mismatch"))),
        }
    }
    let recipients = message
        .recipient_table()
        .ok_or_else(|| invalid("completed store recipient table is missing"))?;
    if recipients.rows_matrix().count() != 0 {
        return Err(invalid("completed store recipient table is not empty"));
    }
    Ok(())
}

fn store_properties(
    spec: &MinimalStore,
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

fn named_property_map() -> Vec<(u16, PropertyValue)> {
    let mut entry = Vec::with_capacity(8);
    entry.extend_from_slice(&0x0000_8005_u32.to_le_bytes());
    entry.extend_from_slice(&0x0002_u16.to_le_bytes());
    entry.extend_from_slice(&0_u16.to_le_bytes());
    let guid = vec![
        0x28, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    vec![
        (0x0001, PropertyValue::Integer32(251)),
        (0x0002, PropertyValue::Binary(guid)),
        (0x0003, PropertyValue::Binary(entry.clone())),
        (0x0004, PropertyValue::Binary(Vec::new())),
        (0x1000, PropertyValue::Binary(entry)),
    ]
}

fn message_properties(spec: &MinimalStore, message_size: i32) -> Vec<(u16, PropertyValue)> {
    const FIXED_FILETIME: i64 = 133_801_632_000_000_000;
    vec![
        (0x001A, PropertyValue::Unicode("IPM.Note".to_owned())),
        (0x0037, PropertyValue::Unicode(spec.subject.clone())),
        (0x0C1A, PropertyValue::Unicode(spec.sender_name.clone())),
        (0x0C1F, PropertyValue::Unicode(spec.sender_email.clone())),
        (0x0E07, PropertyValue::Integer32(1)),
        (0x0E08, PropertyValue::Integer32(message_size)),
        (0x0E17, PropertyValue::Integer32(0)),
        (0x1000, PropertyValue::Unicode(spec.body.clone())),
        (0x3007, PropertyValue::Time(FIXED_FILETIME)),
        (0x3008, PropertyValue::Time(FIXED_FILETIME)),
        (
            0x300B,
            PropertyValue::Binary(b"PSTFORGE-MESSAGE-0001".to_vec()),
        ),
    ]
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
        (PropertyType::Unicode, 0x3704),
        (PropertyType::Integer32, 0x3705),
        (PropertyType::Integer32, 0x370B),
        (PropertyType::Integer32, LTP_ROW_ID_PROP_ID),
        (PropertyType::Integer32, LTP_ROW_VERSION_PROP_ID),
    ])
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

fn message_table_row(id: NodeId, spec: &MinimalStore, message_size: i32) -> TableRowSpec {
    const FIXED_FILETIME: i64 = 133_801_632_000_000_000;
    TableRowSpec {
        id,
        values: vec![
            (0x001A, PropertyValue::Unicode("IPM.Note".to_owned())),
            (0x0037, PropertyValue::Unicode(spec.subject.clone())),
            (0x0E07, PropertyValue::Integer32(1)),
            (0x0E08, PropertyValue::Integer32(message_size)),
            (0x0E30, PropertyValue::Binary(MESSAGE_RECORD_KEY.to_vec())),
            (0x0E33, PropertyValue::Integer64(0x90)),
            (
                0x0E34,
                PropertyValue::Binary(message_instance_entry_id(spec.record_key)),
            ),
            (0x3008, PropertyValue::Time(FIXED_FILETIME)),
        ],
    }
}

fn serialized_message_size(
    spec: &MinimalStore,
    recipient_table: &[u8],
) -> Result<i32, WriterError> {
    let property_bytes = property_context(&message_properties(spec, 0))?;
    let bytes = property_bytes
        .len()
        .checked_add(recipient_table.len())
        .ok_or(WriterError::ValueTooLarge("message size"))?;
    i32::try_from(bytes).map_err(|_| WriterError::ValueTooLarge("message size"))
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
        PropertyValue::Integer32(value) => write_row_bytes(row, offset, &value.to_le_bytes())?,
        PropertyValue::Boolean(value) => write_row_bytes(row, offset, &[u8::from(*value)])?,
        PropertyValue::Integer64(value) | PropertyValue::Time(value) => {
            write_row_bytes(row, offset, &value.to_le_bytes())?
        }
        PropertyValue::Unicode(_) | PropertyValue::Binary(_) => {
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
    }
    mark_column(row, columns, property_id)
}

fn table_variable_bytes(value: &PropertyValue) -> io::Result<Option<Vec<u8>>> {
    let data = match value {
        PropertyValue::Unicode(value) => unicode_bytes(value)?,
        PropertyValue::Binary(value) => value.clone(),
        _ => return Ok(None),
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
            BlockPayload::Data(data) => {
                UnicodeDataBlock::new(NdbCryptMethod::Permute, data.clone(), trailer)
                    .map_err(|error| WriterError::InvalidStructure(error.to_string()))?
                    .write(file)?;
            }
        }
        let physical_size = u64::from(block_size(size.saturating_add(16)));
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

fn nid_counters(entries: &[UnicodeNodeBTreeEntry]) -> Result<[u32; 32], WriterError> {
    let mut counters = INITIAL_NID_COUNTERS;
    for entry in entries {
        let node = entry.node();
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
                continue;
            }
            Err(error) => return Err(WriterError::InvalidStructure(error.to_string())),
        };
        let index = usize::try_from(kind)
            .map_err(|error| WriterError::InvalidStructure(error.to_string()))?;
        let next = node
            .index()
            .checked_add(1)
            .ok_or(WriterError::ValueTooLarge("node counter"))?;
        counters[index] = counters[index].max(next);
    }
    Ok(counters)
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
    use std::{fs::OpenOptions, io::Write as _};

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

        let spec = MinimalStore::default();
        let message = node(NodeIdType::NormalMessage, MESSAGE_INDEX)?;
        let recipients = table_context(&recipient_columns()?, &[])?;
        let message_size = serialized_message_size(&spec, &recipients)?;
        assert_eq!(message_size, 556);
        let row = message_table_row(message, &spec, message_size);
        assert!(row.values.iter().any(|(id, value)| {
            *id == 0x0E33 && matches!(value, PropertyValue::Integer64(0x90))
        }));
        assert!(row.values.iter().any(|(id, value)| {
            *id == 0x0E30
                && matches!(value, PropertyValue::Binary(bytes) if bytes == &MESSAGE_RECORD_KEY)
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
            2, 2, 2, 2, 6, 2, 2, 2, 5, 2, 2, 2, 7, 2, 2, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
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
        assert!(matches!(
            values.get(size_column),
            Some(Some(crate::ltp::table_context::TableRowColumnValue::Small(
                crate::ltp::prop_context::PropertyValue::Integer32(556)
            )))
        ));
        let entry_id = EntryId::new(
            crate::messaging::store::StoreRecordKey::new(spec.record_key),
            message_node,
        );
        let message = store.open_message(&entry_id, None)?;
        assert_eq!(message.properties().message_class()?, "IPM.Note");
        assert_eq!(message.properties().message_size()?, 556);
        let recipients = message
            .recipient_table()
            .ok_or("missing required recipient table")?;
        assert_eq!(recipients.context().columns().len(), 14);
        assert_eq!(recipients.rows_matrix().count(), 0);
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
        let mut temporary = tempfile::NamedTempFile::new_in(directory.path())?;
        temporary.write_all(b"replacement")?;
        temporary.as_file().sync_all()?;

        let error = publish_noclobber(&temporary, &destination)
            .expect_err("atomic publication must not replace an existing destination");
        assert!(matches!(error, WriterError::OutputExists(path) if path == destination));
        assert_eq!(std::fs::read(&destination)?, b"existing");
        assert!(temporary.path().exists());
        Ok(())
    }

    #[test]
    fn durability_error_reports_already_published_output() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("published.pst");
        let missing_parent = directory.path().join("missing");
        let mut temporary = tempfile::NamedTempFile::new_in(directory.path())?;
        temporary.write_all(b"published")?;
        temporary.as_file().sync_all()?;
        publish_noclobber(&temporary, &destination)?;

        let error = sync_published_directory(&destination, &missing_parent)
            .expect_err("missing parent must report uncertain publication durability");
        assert!(matches!(
            error,
            WriterError::PublishedDurability { path, .. } if path == destination
        ));
        assert_eq!(std::fs::read(&destination)?, b"published");
        assert!(!temporary.path().exists());
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
}
