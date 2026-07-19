//! ## [Attachment Objects](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/46eb4828-c6a5-420d-a137-9ee36df317c1)

use std::{collections::BTreeMap, io, rc::Rc};

use super::{message::*, read_write::*, *};
use crate::{
    AnsiPstFile, PstFile, PstFileLock, UnicodePstFile,
    ltp::{
        heap::HeapNode,
        prop_context::{BinaryValue, PropertyContext, PropertyValue},
        prop_type::PropertyType,
        read_write::*,
    },
    ndb::{
        block::{DataTree, IntermediateTreeBlock, SubNodeTree},
        block_id::BlockId,
        header::Header,
        node_id::{NodeId, NodeIdType},
        page::{BTreePage, NodeBTreeEntry, RootBTree},
        read_write::*,
        root::Root,
    },
};

#[derive(Default, Debug)]
pub struct AttachmentProperties {
    properties: BTreeMap<u16, PropertyValue>,
}

impl AttachmentProperties {
    pub fn get(&self, id: u16) -> Option<&PropertyValue> {
        self.properties.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u16, &PropertyValue)> {
        self.properties.iter()
    }

    pub fn attachment_size(&self) -> io::Result<i32> {
        let attachment_size = self
            .properties
            .get(&0x0E20)
            .ok_or(MessagingError::AttachmentSizeNotFound)?;

        match attachment_size {
            PropertyValue::Integer32(value) => Ok(*value),
            invalid => {
                Err(MessagingError::InvalidAttachmentSize(PropertyType::from(invalid)).into())
            }
        }
    }

    pub fn attachment_method(&self) -> io::Result<i32> {
        let attachment_method = self
            .properties
            .get(&0x3705)
            .ok_or(MessagingError::AttachmentMethodNotFound)?;

        match attachment_method {
            PropertyValue::Integer32(value) => Ok(*value),
            invalid => {
                Err(MessagingError::InvalidAttachmentMethod(PropertyType::from(invalid)).into())
            }
        }
    }

    pub fn rendering_position(&self) -> io::Result<i32> {
        let rendering_position = self
            .properties
            .get(&0x370B)
            .ok_or(MessagingError::AttachmentRenderingPositionNotFound)?;

        match rendering_position {
            PropertyValue::Integer32(value) => Ok(*value),
            invalid => Err(
                MessagingError::InvalidAttachmentRenderingPosition(PropertyType::from(invalid))
                    .into(),
            ),
        }
    }
}

fn declared_attachment_size(properties: &AttachmentProperties) -> io::Result<usize> {
    let size = properties.attachment_size()?;
    usize::try_from(size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "attachment size is negative"))
}

fn validate_attachment_content_size(declared: usize, actual: usize) -> io::Result<()> {
    if actual > declared {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "attachment payload exceeds its declared object size",
        ))
    } else {
        Ok(())
    }
}

fn validate_object_content_size(declared: u32, actual: usize) -> io::Result<()> {
    if declared as usize != actual {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "attachment object size does not match its referenced data",
        ))
    } else {
        Ok(())
    }
}

/// [PidTagAttachMethod](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcmsg/252923d6-dd41-468b-9c57-d3f68051a516)
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum AttachmentMethod {
    /// `afNone`: The attachment has just been created.
    #[default]
    None = 0x00000000,
    /// `afByValue`: The `PidTagAttachDataBinary` property (section [2.2.2.7](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcmsg/42dfb62b-2ff5-4ffc-ae25-bfdd2db3d8e0))
    /// contains the attachment data.
    ByValue = 0x00000001,
    /// `afByReference`: The `PidTagAttachLongPathname` property (section [2.2.2.13](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcmsg/74b1b39e-1cb4-48ad-b28e-405a261e556c))
    /// contains a fully qualified path identifying the attachment To recipients with access to a
    /// common file server.
    ByReference = 0x00000002,
    /// Legacy `ATTACH_BY_REF_RESOLVE`: a data-less reference whose long
    /// pathname is resolved by the consuming MAPI implementation.
    ByReferenceResolve = 0x00000003,
    /// `afByReferenceOnly`: The `PidTagAttachLongPathname` property contains a fully qualified
    /// path identifying the attachment.
    ByReferenceOnly = 0x00000004,
    /// `afEmbeddedMessage`: The attachment is an embedded message that is accessed via the `RopOpenEmbeddedMessage` ROP ([MS-OXCROPS](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcrops/13af6911-27e5-4aa0-bb75-637b02d4f2ef)
    /// section [2.2.6.16](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcrops/bce79473-e082-4452-822c-ab8cb055dee6)).
    EmbeddedMessage = 0x00000005,
    /// `afStorage`: The `PidTagAttachDataObject` property (section [2.2.2.8](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcmsg/0691206f-0082-463a-a12f-58cb7cb7875f))
    /// contains data in an application-specific format.
    Storage = 0x00000006,
    /// `afByWebReference`: The `PidTagAttachLongPathname` property contains a fully qualified path
    /// identifying the attachment. The `PidNameAttachmentProviderType` defines the web service API
    /// manipulating the attachment.
    ByWebReference = 0x00000007,
}

impl TryFrom<i32> for AttachmentMethod {
    type Error = MessagingError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0x00000000 => Ok(Self::None),
            0x00000001 => Ok(Self::ByValue),
            0x00000002 => Ok(Self::ByReference),
            0x00000003 => Ok(Self::ByReferenceResolve),
            0x00000004 => Ok(Self::ByReferenceOnly),
            0x00000005 => Ok(Self::EmbeddedMessage),
            0x00000006 => Ok(Self::Storage),
            0x00000007 => Ok(Self::ByWebReference),
            _ => Err(MessagingError::UnknownAttachmentMethod(value)),
        }
    }
}

pub enum AttachmentData {
    Binary(BinaryValue),
    Message(Rc<dyn Message>),
}

pub trait Attachment {
    fn message(&self) -> Rc<dyn Message>;
    fn properties(&self) -> &AttachmentProperties;
    fn data(&self) -> Option<&AttachmentData>;
    fn streamed_data_identity(&self) -> Option<(u64, [u8; 32])>;
}

struct AttachmentInner<Pst>
where
    Pst: PstFile,
{
    message: Rc<Pst::Message>,
    properties: AttachmentProperties,
    data: Option<AttachmentData>,
    streamed_data_identity: Option<(u64, [u8; 32])>,
}

impl<Pst> AttachmentInner<Pst>
where
    Pst: PstFile + PstFileLock<Pst>,
    <Pst as PstFile>::BTreeKey: BTreePageKeyReadWrite,
    <Pst as PstFile>::NodeBTreeEntry: NodeBTreeEntryReadWrite,
    <Pst as PstFile>::NodeBTree: RootBTreeReadWrite,
    <<Pst as PstFile>::NodeBTree as RootBTree>::IntermediatePage:
        RootBTreeIntermediatePageReadWrite<
                Pst,
                <Pst as PstFile>::NodeBTreeEntry,
                <<Pst as PstFile>::NodeBTree as RootBTree>::LeafPage,
            >,
    <<<Pst as PstFile>::NodeBTree as RootBTree>::IntermediatePage as BTreePage>::Entry:
        BTreePageEntryReadWrite,
    <<Pst as PstFile>::NodeBTree as RootBTree>::LeafPage: RootBTreeLeafPageReadWrite<Pst>,
    <Pst as PstFile>::BlockBTreeEntry: BlockBTreeEntryReadWrite,
    <Pst as PstFile>::BlockBTree: RootBTreeReadWrite,
    <<Pst as PstFile>::BlockBTree as RootBTree>::Entry: BTreeEntryReadWrite,
    <<Pst as PstFile>::BlockBTree as RootBTree>::IntermediatePage:
        RootBTreeIntermediatePageReadWrite<
                Pst,
                <<Pst as PstFile>::BlockBTree as RootBTree>::Entry,
                <<Pst as PstFile>::BlockBTree as RootBTree>::LeafPage,
            >,
    <<Pst as PstFile>::BlockBTree as RootBTree>::LeafPage:
        RootBTreeLeafPageReadWrite<Pst> + BTreePageReadWrite,
    <Pst as PstFile>::BlockTrailer: BlockTrailerReadWrite,
    <Pst as PstFile>::SubNodeTreeBlockHeader: IntermediateTreeHeaderReadWrite,
    <Pst as PstFile>::SubNodeTreeBlock: IntermediateTreeBlockReadWrite,
    <<Pst as PstFile>::SubNodeTreeBlock as IntermediateTreeBlock>::Entry:
        IntermediateTreeEntryReadWrite,
    <Pst as PstFile>::SubNodeBlock: IntermediateTreeBlockReadWrite,
    <<Pst as PstFile>::SubNodeBlock as IntermediateTreeBlock>::Entry:
        IntermediateTreeEntryReadWrite,
    <Pst as PstFile>::DataTreeBlock: IntermediateTreeBlockReadWrite,
    <<Pst as PstFile>::DataTreeBlock as IntermediateTreeBlock>::Entry:
        IntermediateTreeEntryReadWrite,
    <Pst as PstFile>::DataBlock: BlockReadWrite + Clone,
    <Pst as PstFile>::HeapNode: HeapNodeReadWrite<Pst>,
    <Pst as PstFile>::PropertyTree: HeapTreeReadWrite<Pst>,
    <Pst as PstFile>::PropertyContext: PropertyContextReadWrite<Pst>,
    <Pst as PstFile>::Store: StoreReadWrite<Pst>,
    <Pst as PstFile>::Message: MessageReadWrite<Pst> + 'static,
{
    fn read(
        message: Rc<<Pst as PstFile>::Message>,
        sub_node: NodeId,
        prop_ids: Option<&[u16]>,
        materialize_data: bool,
        embedded_streamed_ids: &[u16],
    ) -> io::Result<Self> {
        let node_id_type = sub_node.id_type()?;
        match node_id_type {
            NodeIdType::Attachment => {}
            _ => {
                return Err(MessagingError::InvalidAttachmentNodeIdType(node_id_type).into());
            }
        }

        let store = message.pst_store();
        let property_budget = message.materialization_budget().clone();
        let pst = store.pst();
        let header = pst.header();
        let root = header.root();

        let (properties, data, embedded_node, streamed_data_identity) = {
            let mut file = pst
                .reader()
                .lock()
                .map_err(|_| MessagingError::FailedToLockFile)?;
            let file = &mut *file;

            let encoding = header.crypt_method();
            let block_btree = <<Pst as PstFile>::BlockBTree as RootBTreeReadWrite>::read(
                file,
                *root.block_btree(),
            )?;

            let node = message
                .sub_nodes()
                .get(&sub_node)
                .ok_or(MessagingError::AttachmentSubNodeNotFound(sub_node))?;
            let node = <<Pst as PstFile>::NodeBTreeEntry as NodeBTreeEntryReadWrite>::new(
                node.node(),
                node.block(),
                node.sub_node(),
                None,
            );
            let mut page_cache = pst.block_cache();
            let data = node.data();
            let heap = <<Pst as PstFile>::HeapNode as HeapNodeReadWrite<Pst>>::read(
                file,
                &block_btree,
                &mut page_cache,
                encoding,
                data.search_key(),
            )?;
            let header = heap.header()?;

            let tree = <Pst as PstFile>::PropertyTree::new(heap, header.user_root());
            let prop_context = <<Pst as PstFile>::PropertyContext as PropertyContextReadWrite<
                Pst,
            >>::new(node, tree);
            let mut properties = BTreeMap::new();
            let mut streamed_data_identity = None;
            for (prop_id, record) in prop_context.properties()? {
                if !materialize_data && prop_id == 0x3701 {
                    if record.prop_type() != PropertyType::Binary {
                        return Err(
                            MessagingError::InvalidMessageObjectData(record.prop_type()).into()
                        );
                    }
                    let (_, byte_len, sha256) = prop_context.stream_property_identity(
                        file,
                        encoding,
                        &block_btree,
                        &mut page_cache,
                        record,
                    )?;
                    streamed_data_identity = Some((byte_len, sha256));
                    continue;
                }
                let value = prop_context.read_property(
                    file,
                    encoding,
                    &block_btree,
                    &mut page_cache,
                    record,
                    Some(&mut *property_budget.borrow_mut()),
                )?;
                properties.insert(prop_id, value);
            }
            let properties = AttachmentProperties { properties };
            let attachment_size = declared_attachment_size(&properties)?;
            let attachment_method = AttachmentMethod::try_from(properties.attachment_method()?)?;
            let data = if !materialize_data {
                (None, None)
            } else {
                match attachment_method {
                    AttachmentMethod::ByValue => {
                        let binary_data = match properties
                            .get(0x3701)
                            .ok_or(MessagingError::AttachmentMessageObjectDataNotFound)?
                        {
                            PropertyValue::Binary(value) => value.clone(),
                            PropertyValue::Null => BinaryValue::new(Vec::new()),
                            invalid => {
                                return Err(MessagingError::InvalidMessageObjectData(
                                    PropertyType::from(invalid),
                                )
                                .into());
                            }
                        };
                        validate_attachment_content_size(
                            attachment_size,
                            binary_data.buffer().len(),
                        )?;
                        (Some(AttachmentData::Binary(binary_data)), None)
                    }
                    AttachmentMethod::EmbeddedMessage => {
                        let object_data = match properties
                            .get(0x3701)
                            .ok_or(MessagingError::AttachmentMessageObjectDataNotFound)?
                        {
                            PropertyValue::Object(value) => value,
                            invalid => {
                                return Err(MessagingError::InvalidMessageObjectData(
                                    PropertyType::from(invalid),
                                )
                                .into());
                            }
                        };
                        let sub_node = object_data.node();
                        let root = node
                            .sub_node()
                            .ok_or(MessagingError::AttachmentSubNodeNotFound(sub_node))?;
                        let block =
                            block_btree.find_entry(file, root.search_key(), &mut page_cache)?;
                        let tree = SubNodeTree::<Pst>::read(file, &block)?;
                        let node = tree.find_leaf_entry_bounded(
                            file,
                            &block_btree,
                            sub_node,
                            &mut page_cache,
                        )?;
                        let node =
                            <<Pst as PstFile>::NodeBTreeEntry as NodeBTreeEntryReadWrite>::new(
                                node.node(),
                                node.block(),
                                node.sub_node(),
                                None,
                            );
                        (None, Some((node, attachment_size, object_data.size())))
                    }
                    AttachmentMethod::Storage => {
                        let object_data = match properties
                            .get(0x3701)
                            .ok_or(MessagingError::AttachmentMessageObjectDataNotFound)?
                        {
                            PropertyValue::Object(value) => value,
                            invalid => {
                                return Err(MessagingError::InvalidMessageObjectData(
                                    PropertyType::from(invalid),
                                )
                                .into());
                            }
                        };
                        let object_size = object_data.size();
                        let sub_node = object_data.node();
                        let root = node
                            .sub_node()
                            .ok_or(MessagingError::AttachmentSubNodeNotFound(sub_node))?;
                        let block =
                            block_btree.find_entry(file, root.search_key(), &mut page_cache)?;
                        let tree = SubNodeTree::<Pst>::read(file, &block)?;
                        let node = tree.find_leaf_entry_bounded(
                            file,
                            &block_btree,
                            sub_node,
                            &mut page_cache,
                        )?;
                        let block = block_btree.find_entry(
                            file,
                            node.block().search_key(),
                            &mut page_cache,
                        )?;
                        let block = DataTree::read(file, encoding, &block)?;
                        property_budget.borrow_mut().charge(block.declared_size())?;
                        let mut data = vec![];
                        let _ = block
                            .reader(
                                file,
                                encoding,
                                &block_btree,
                                &mut page_cache,
                                &mut Default::default(),
                            )?
                            .read_to_end(&mut data)?;
                        validate_object_content_size(object_size, data.len())?;
                        validate_attachment_content_size(attachment_size, data.len())?;
                        (Some(AttachmentData::Binary(BinaryValue::new(data))), None)
                    }
                    _ => (None, None),
                }
            };

            (properties, data.0, data.1, streamed_data_identity)
        };
        let data = match embedded_node {
            Some((node, attachment_size, object_size)) => {
                let message = <<Pst as PstFile>::Message as MessageReadWrite<
                    Pst,
                >>::read_embedded_with_streamed_properties(
                    store.clone(),
                    node,
                    prop_ids,
                    embedded_streamed_ids,
                    property_budget.clone(),
                )?;
                let message_size =
                    usize::try_from(message.properties().message_size()?).map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "embedded message size is negative",
                        )
                    })?;
                validate_object_content_size(object_size, message_size)?;
                validate_attachment_content_size(attachment_size, message_size)?;
                Some(AttachmentData::Message(message))
            }
            None => data,
        };

        Ok(Self {
            message,
            properties,
            data,
            streamed_data_identity,
        })
    }
}

pub struct UnicodeAttachment {
    inner: AttachmentInner<UnicodePstFile>,
}

impl UnicodeAttachment {
    pub fn read(
        message: Rc<UnicodeMessage>,
        sub_node: NodeId,
        prop_ids: Option<&[u16]>,
    ) -> io::Result<Rc<Self>> {
        <Self as AttachmentReadWrite<UnicodePstFile>>::read(message, sub_node, prop_ids)
    }

    /// Read attachment metadata without materializing its by-value payload.
    pub fn read_metadata(message: Rc<UnicodeMessage>, sub_node: NodeId) -> io::Result<Rc<Self>> {
        let inner = AttachmentInner::read(message, sub_node, None, false, &[])?;
        Ok(Rc::new(Self { inner }))
    }

    pub fn read_with_streamed_embedded_properties(
        message: Rc<UnicodeMessage>,
        sub_node: NodeId,
        prop_ids: Option<&[u16]>,
        streamed_ids: &[u16],
    ) -> io::Result<Rc<Self>> {
        let inner = AttachmentInner::read(message, sub_node, prop_ids, true, streamed_ids)?;
        Ok(Rc::new(Self { inner }))
    }

    pub fn streamed_data_identity(&self) -> Option<(u64, [u8; 32])> {
        self.inner.streamed_data_identity
    }
}

impl Attachment for UnicodeAttachment {
    fn message(&self) -> Rc<dyn Message> {
        self.inner.message.clone()
    }

    fn properties(&self) -> &AttachmentProperties {
        &self.inner.properties
    }

    fn data(&self) -> Option<&AttachmentData> {
        self.inner.data.as_ref()
    }

    fn streamed_data_identity(&self) -> Option<(u64, [u8; 32])> {
        self.inner.streamed_data_identity
    }
}

impl AttachmentReadWrite<UnicodePstFile> for UnicodeAttachment {
    fn read(
        message: Rc<UnicodeMessage>,
        sub_node: NodeId,
        prop_ids: Option<&[u16]>,
    ) -> io::Result<Rc<Self>> {
        let inner = AttachmentInner::read(message, sub_node, prop_ids, true, &[])?;
        Ok(Rc::new(Self { inner }))
    }
}

pub struct AnsiAttachment {
    inner: AttachmentInner<AnsiPstFile>,
}

impl AnsiAttachment {
    pub fn read(
        message: Rc<AnsiMessage>,
        sub_node: NodeId,
        prop_ids: Option<&[u16]>,
    ) -> io::Result<Rc<Self>> {
        <Self as AttachmentReadWrite<AnsiPstFile>>::read(message, sub_node, prop_ids)
    }

    /// Read attachment metadata without materializing its by-value payload.
    pub fn read_metadata(message: Rc<AnsiMessage>, sub_node: NodeId) -> io::Result<Rc<Self>> {
        let inner = AttachmentInner::read(message, sub_node, None, false, &[])?;
        Ok(Rc::new(Self { inner }))
    }

    pub fn streamed_data_identity(&self) -> Option<(u64, [u8; 32])> {
        self.inner.streamed_data_identity
    }
}

impl Attachment for AnsiAttachment {
    fn message(&self) -> Rc<dyn Message> {
        self.inner.message.clone()
    }

    fn properties(&self) -> &AttachmentProperties {
        &self.inner.properties
    }

    fn data(&self) -> Option<&AttachmentData> {
        self.inner.data.as_ref()
    }

    fn streamed_data_identity(&self) -> Option<(u64, [u8; 32])> {
        self.inner.streamed_data_identity
    }
}

impl AttachmentReadWrite<AnsiPstFile> for AnsiAttachment {
    fn read(
        message: Rc<AnsiMessage>,
        sub_node: NodeId,
        prop_ids: Option<&[u16]>,
    ) -> io::Result<Rc<Self>> {
        let inner = AttachmentInner::read(message, sub_node, prop_ids, true, &[])?;
        Ok(Rc::new(Self { inner }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ndb::block::{
        DataTreeBlockHeader, UnicodeBlockTrailer, UnicodeDataTreeBlock, UnicodeDataTreeEntry,
    };
    use crate::ndb::block_id::UnicodeBlockId;
    use crate::ndb::read_write::IntermediateTreeBlockReadWrite;

    #[test]
    fn attachment_size_validation_rejects_negative_and_undersized_values() {
        let negative = AttachmentProperties {
            properties: BTreeMap::from([(0x0E20, PropertyValue::Integer32(-1))]),
        };
        assert!(declared_attachment_size(&negative).is_err());
        assert!(validate_attachment_content_size(4, 5).is_err());
        assert!(validate_attachment_content_size(4, 4).is_ok());
        assert!(validate_attachment_content_size(4, 3).is_ok());
        assert!(validate_object_content_size(4, 3).is_err());
        assert!(validate_object_content_size(4, 4).is_ok());
    }

    #[test]
    fn embedded_object_size_uses_xblock_logical_size() {
        let root = UnicodeBlockId::new(true, 1).expect("internal block ID");
        let leaf = UnicodeBlockId::new(false, 2).expect("external block ID");
        let trailer = UnicodeBlockTrailer::new(16, 0, 0, root).expect("XBLOCK trailer");
        let xblock = UnicodeDataTreeBlock::new(
            DataTreeBlockHeader::new(1, 1, 16_384),
            vec![UnicodeDataTreeEntry::from(leaf)],
            trailer,
        )
        .expect("valid XBLOCK");
        let tree = DataTree::<UnicodePstFile>::Intermediate(Box::new(xblock));

        assert_eq!(tree.declared_size(), 16_384);
        assert!(validate_object_content_size(16_384, tree.declared_size()).is_ok());
        assert!(validate_object_content_size(16_384, 16).is_err());
    }
}
