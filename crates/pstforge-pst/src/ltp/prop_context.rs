//! ## [Property Context (PC)](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/294c83c6-ff92-42f5-b6b6-876c29fa9737)

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use core::mem;
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fmt::{Debug, Display},
    io::{self, Cursor, Read, Write},
    rc::Rc,
};

use super::{heap::*, prop_type::*, read_write::*, tree::*, *};
use crate::{
    AnsiPstFile, PstFile, PstFileReadWriteBlockBTree, PstReader, UnicodePstFile,
    ndb::{
        block::{DataBlockCache, DataTree, IntermediateTreeBlock, SubNodeTree},
        block_id::BlockId,
        block_ref::BlockRef,
        header::NdbCryptMethod,
        node_id::{NodeId, NodeIdType},
        page::{
            AnsiBlockBTree, AnsiNodeBTreeEntry, BlockBTreeEntry, NodeBTreeEntry, RootBTree,
            UnicodeBlockBTree, UnicodeNodeBTreeEntry,
        },
        read_write::*,
    },
};

#[derive(Copy, Clone)]
pub enum PropertyValueRecord {
    Small(u32),
    Heap(HeapId),
    Node(NodeId),
}

impl PropertyValueRecord {
    pub fn small_value(&self, prop_type: PropertyType) -> Option<PropertyValue> {
        match (self, prop_type) {
            (PropertyValueRecord::Small(value), PropertyType::Integer16) => {
                Some(PropertyValue::Integer16((*value & 0xFFFF) as i16))
            }
            (PropertyValueRecord::Small(value), PropertyType::Integer32) => {
                Some(PropertyValue::Integer32(*value as i32))
            }
            (PropertyValueRecord::Small(value), PropertyType::Floating32) => {
                Some(PropertyValue::Floating32(f32::from_bits(*value)))
            }
            (PropertyValueRecord::Small(value), PropertyType::ErrorCode) => {
                Some(PropertyValue::ErrorCode(*value as i32))
            }
            (PropertyValueRecord::Small(value), PropertyType::Boolean) => {
                Some(PropertyValue::Boolean(*value & 0xFF != 0))
            }
            _ => None,
        }
    }
}

impl Debug for PropertyValueRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PropertyValueRecord::Small(value) => write!(f, "Small(0x{value:08X})"),
            PropertyValueRecord::Heap(heap_id) => write!(f, "{heap_id:?}"),
            PropertyValueRecord::Node(node_id) => write!(f, "{node_id:?}"),
        }
    }
}

impl From<PropertyValueRecord> for u32 {
    fn from(value: PropertyValueRecord) -> Self {
        match value {
            PropertyValueRecord::Small(value) => value,
            PropertyValueRecord::Heap(heap_id) => u32::from(heap_id),
            PropertyValueRecord::Node(node_id) => u32::from(node_id),
        }
    }
}

pub type PropertyTreeRecordKey = u16;

impl HeapTreeEntryKey for PropertyTreeRecordKey {
    const SIZE: u8 = 2;
}

impl HeapNodePageReadWrite for PropertyTreeRecordKey {
    fn read(f: &mut dyn Read) -> io::Result<Self> {
        f.read_u16::<LittleEndian>()
    }

    fn write(&self, f: &mut dyn Write) -> io::Result<()> {
        f.write_u16::<LittleEndian>(*self)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PropertyTreeRecordValue {
    prop_type: PropertyType,
    value: PropertyValueRecord,
}

impl PropertyTreeRecordValue {
    pub fn new(prop_type: PropertyType, value: PropertyValueRecord) -> Self {
        Self { prop_type, value }
    }

    pub fn prop_type(&self) -> PropertyType {
        self.prop_type
    }

    pub fn value(&self) -> PropertyValueRecord {
        self.value
    }
}

impl HeapTreeEntryValue for PropertyTreeRecordValue {
    const SIZE: u8 = 6;
}

impl HeapNodePageReadWrite for PropertyTreeRecordValue {
    fn read(f: &mut dyn Read) -> io::Result<Self> {
        // wPropType
        let prop_type = f.read_u16::<LittleEndian>()?;
        let prop_type = PropertyType::try_from(prop_type)?;

        // dwValueHnid
        let value = f.read_u32::<LittleEndian>()?;
        let value = match prop_type {
            PropertyType::Null => PropertyValueRecord::Small(0),

            PropertyType::Integer16 => PropertyValueRecord::Small(value & 0xFFFF),

            PropertyType::Integer32 | PropertyType::Floating32 | PropertyType::ErrorCode => {
                PropertyValueRecord::Small(value)
            }

            PropertyType::Boolean => PropertyValueRecord::Small(value & 0xFF),

            PropertyType::Floating64
            | PropertyType::Currency
            | PropertyType::FloatingTime
            | PropertyType::Integer64
            | PropertyType::Time
            | PropertyType::Guid
            | PropertyType::Object => PropertyValueRecord::Heap(HeapId::from(value)),

            _ => match NodeId::from(value).id_type() {
                Ok(NodeIdType::HeapNode) => PropertyValueRecord::Heap(HeapId::from(value)),
                _ => PropertyValueRecord::Node(NodeId::from(value)),
            },
        };

        Ok(Self { prop_type, value })
    }

    fn write(&self, f: &mut dyn Write) -> io::Result<()> {
        f.write_u16::<LittleEndian>(u16::from(self.prop_type))?;
        f.write_u32::<LittleEndian>(u32::from(self.value))
    }
}

/// [PC BTH Record](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/7daab6f5-ce65-437e-80d5-1b1be4088bd3)
#[derive(Clone, Copy)]
pub struct PropertyTreeRecord {
    key: PropertyTreeRecordKey,
    data: PropertyTreeRecordValue,
}

impl PropertyTreeRecord {
    pub fn new(prop_id: u16, prop_type: PropertyType, value: PropertyValueRecord) -> Self {
        Self {
            key: prop_id,
            data: PropertyTreeRecordValue::new(prop_type, value),
        }
    }

    pub fn prop_id(&self) -> u16 {
        self.key
    }

    pub fn prop_type(&self) -> PropertyType {
        self.data.prop_type()
    }

    pub fn value(&self) -> PropertyValueRecord {
        self.data.value()
    }
}

impl PropertyTreeRecordReadWrite for PropertyTreeRecord {
    fn read(f: &mut dyn Read) -> io::Result<Self> {
        let key = f.read_u16::<LittleEndian>()?;
        let data = PropertyTreeRecordValue::read(f)?;

        Ok(Self { key, data })
    }

    fn write(&self, f: &mut dyn Write) -> io::Result<()> {
        f.write_u16::<LittleEndian>(self.key)?;
        self.data.write(f)
    }
}

#[derive(Clone, Default)]
pub struct String8Value {
    buffer: Vec<u8>,
}

impl String8Value {
    pub fn buffer(&self) -> &[u8] {
        &self.buffer
    }
}

impl Display for String8Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let buffer: Vec<_> = self.buffer.iter().map(|&b| u16::from(b)).collect();
        let value = String::from_utf16_lossy(&buffer);
        write!(f, "{value}")
    }
}

impl Debug for String8Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self.to_string();
        write!(f, "String8Value {{ {value:?} }}")
    }
}

#[derive(Clone, Default)]
pub struct UnicodeValue {
    buffer: Vec<u16>,
}

impl UnicodeValue {
    pub fn buffer(&self) -> &[u16] {
        &self.buffer
    }
}

impl Display for UnicodeValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = String::from_utf16_lossy(&self.buffer);
        write!(f, "{value}")
    }
}

impl Debug for UnicodeValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self.to_string();
        write!(f, "UnicodeValue {{ {value:?} }}")
    }
}

#[derive(Clone, Copy, Default)]
pub struct GuidValue {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl GuidValue {
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }

    pub fn to_le_bytes(self) -> [u8; 16] {
        let mut bytes = [0_u8; 16];
        bytes[..4].copy_from_slice(&self.data1.to_le_bytes());
        bytes[4..6].copy_from_slice(&self.data2.to_le_bytes());
        bytes[6..8].copy_from_slice(&self.data3.to_le_bytes());
        bytes[8..].copy_from_slice(&self.data4);
        bytes
    }

    pub fn data1(&self) -> u32 {
        self.data1
    }

    pub fn data2(&self) -> u16 {
        self.data2
    }

    pub fn data3(&self) -> u16 {
        self.data3
    }

    pub fn data4(&self) -> &[u8; 8] {
        &self.data4
    }
}

impl Debug for GuidValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GuidValue {{ {:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X} }}",
            self.data1,
            self.data2,
            self.data3,
            self.data4[0],
            self.data4[1],
            self.data4[2],
            self.data4[3],
            self.data4[4],
            self.data4[5],
            self.data4[6],
            self.data4[7]
        )
    }
}

#[derive(Clone, Default)]
pub struct BinaryValue {
    buffer: Rc<[u8]>,
}

impl BinaryValue {
    pub fn new(buffer: Vec<u8>) -> Self {
        Self {
            buffer: buffer.into(),
        }
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buffer
    }
}

impl Debug for BinaryValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self
            .buffer
            .iter()
            .map(|ch| format!("{ch:02X}"))
            .collect::<Vec<_>>()
            .join("-");
        write!(f, "BinaryValue {{ {value} }}")
    }
}

#[derive(Clone, Copy, Default)]
pub struct ObjectValue {
    node_id: NodeId,
    size: u32,
}

impl ObjectValue {
    pub fn node(&self) -> NodeId {
        self.node_id
    }

    pub fn size(&self) -> u32 {
        self.size
    }
}

impl Debug for ObjectValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ObjectValue {{ {:?}, size: 0x{:X} }}",
            self.node_id, self.size
        )
    }
}

#[derive(Clone, Default, Debug)]
pub enum PropertyValue {
    /// `PtypNull`: None: This property is a placeholder.
    #[default]
    Null,
    /// `PtypInteger16`: 2 bytes; a 16-bit integer
    Integer16(i16),
    /// `PtypInteger32`: 4 bytes; a 32-bit integer
    Integer32(i32),
    /// `PtypFloating32`: 4 bytes; a 32-bit floating-point number
    Floating32(f32),
    /// `PtypFloating64`: 8 bytes; a 64-bit floating-point number
    Floating64(f64),
    /// `PtypCurrency`: 8 bytes; a 64-bit signed, scaled integer representation of a decimal
    /// currency value, with four places to the right of the decimal point
    Currency(i64),
    /// `PtypFloatingTime`: 8 bytes; a 64-bit floating point number in which the whole number part
    /// represents the number of days since December 30, 1899, and the fractional part represents
    /// the fraction of a day since midnight
    FloatingTime(f64),
    /// `PtypErrorCode`: 4 bytes; a 32-bit integer encoding error information as specified in
    /// section [2.4.1](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcdata/c9dc2fb0-73ca-4cc2-bdee-cc6ffb9b70eb).
    ErrorCode(i32),
    /// `PtypBoolean`: 1 byte; restricted to 1 or 0
    Boolean(bool),
    /// `PtypInteger64`: 8 bytes; a 64-bit integer
    Integer64(i64),
    /// `PtypString8`: Variable size; a string of multibyte characters in externally specified
    /// encoding with terminating null character (single 0 byte).
    String8(String8Value),
    /// `PtypString`: Variable size; a string of Unicode characters in UTF-16LE format encoding
    /// with terminating null character (0x0000).
    Unicode(UnicodeValue),
    /// `PtypTime`: 8 bytes; a 64-bit integer representing the number of 100-nanosecond intervals
    /// since January 1, 1601
    Time(i64),
    /// `PtypGuid`: 16 bytes; a GUID with Data1, Data2, and Data3 fields in little-endian format
    Guid(GuidValue),
    /// `PtypBinary`: Variable size; a COUNT field followed by that many bytes.
    Binary(BinaryValue),
    /// `PtypObject`: The property value is a Component Object Model (COM) object, as specified in
    /// section [2.11.1.5](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcdata/5a024c95-2264-4832-9840-d6260c9c2cdb).
    Object(ObjectValue),

    /// `PtypMultipleInteger16`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::Integer16] values.
    MultipleInteger16(Vec<i16>),
    /// `PtypMultipleInteger32`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::Integer32] values.
    MultipleInteger32(Vec<i32>),
    /// `PtypMultipleFloating32`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::Floating32] values.
    MultipleFloating32(Vec<f32>),
    /// `PtypFloating64`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::Floating64] values.
    MultipleFloating64(Vec<f64>),
    /// `PtypMultipleCurrency`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::Currency] values.
    MultipleCurrency(Vec<i64>),
    /// `PtypMultipleFloatingTime`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::FloatingTime] values.
    MultipleFloatingTime(Vec<f64>),
    /// `PtypMultipleInteger64`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::Integer64] values.
    MultipleInteger64(Vec<i64>),
    /// `PtypMultipleString8`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::String8] values.
    MultipleString8(Vec<String8Value>),
    /// `PtypMultipleString`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::Unicode] values.
    MultipleUnicode(Vec<UnicodeValue>),
    /// `PtypMultipleTime`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::Time] values.
    MultipleTime(Vec<i64>),
    /// `PtypMultipleGuid`: packed fixed-width [PropertyValue::Guid] values.
    MultipleGuid(Vec<GuidValue>),
    /// `PtypMultipleBinary`: Variable size; a COUNT field followed by that many
    /// [PropertyValue::Binary] values.
    MultipleBinary(Vec<BinaryValue>),
}

impl PropertyValue {
    pub(crate) fn materialized_size(&self) -> Option<usize> {
        let base = std::mem::size_of::<Self>();
        let extra = match self {
            Self::String8(value) => value.buffer.capacity(),
            Self::Unicode(value) => value
                .buffer
                .capacity()
                .checked_mul(std::mem::size_of::<u16>())?,
            Self::Binary(value) => value.buffer.len(),
            Self::MultipleInteger16(values) => {
                values.capacity().checked_mul(std::mem::size_of::<i16>())?
            }
            Self::MultipleInteger32(values) => {
                values.capacity().checked_mul(std::mem::size_of::<i32>())?
            }
            Self::MultipleFloating32(values) => {
                values.capacity().checked_mul(std::mem::size_of::<f32>())?
            }
            Self::MultipleFloating64(values) => {
                values.capacity().checked_mul(std::mem::size_of::<f64>())?
            }
            Self::MultipleCurrency(values) => {
                values.capacity().checked_mul(std::mem::size_of::<i64>())?
            }
            Self::MultipleFloatingTime(values) => {
                values.capacity().checked_mul(std::mem::size_of::<f64>())?
            }
            Self::MultipleInteger64(values) => {
                values.capacity().checked_mul(std::mem::size_of::<i64>())?
            }
            Self::MultipleTime(values) => {
                values.capacity().checked_mul(std::mem::size_of::<i64>())?
            }
            Self::MultipleGuid(values) => values
                .capacity()
                .checked_mul(std::mem::size_of::<GuidValue>())?,
            Self::MultipleString8(values) => values.iter().try_fold(
                values
                    .capacity()
                    .checked_mul(std::mem::size_of::<String8Value>())?,
                |total, value| total.checked_add(value.buffer.len()),
            )?,
            Self::MultipleUnicode(values) => values.iter().try_fold(
                values
                    .capacity()
                    .checked_mul(std::mem::size_of::<UnicodeValue>())?,
                |total, value| {
                    total.checked_add(
                        value
                            .buffer
                            .capacity()
                            .checked_mul(std::mem::size_of::<u16>())?,
                    )
                },
            )?,
            Self::MultipleBinary(values) => values.iter().try_fold(
                values
                    .capacity()
                    .checked_mul(std::mem::size_of::<BinaryValue>())?,
                |total, value| total.checked_add(value.buffer.len()),
            )?,
            _ => 0,
        };
        base.checked_add(extra)
    }
}

impl From<&PropertyValue> for PropertyType {
    fn from(value: &PropertyValue) -> Self {
        match value {
            PropertyValue::Null => PropertyType::Null,
            PropertyValue::Integer16(_) => PropertyType::Integer16,
            PropertyValue::Integer32(_) => PropertyType::Integer32,
            PropertyValue::Floating32(_) => PropertyType::Floating32,
            PropertyValue::Floating64(_) => PropertyType::Floating64,
            PropertyValue::Currency(_) => PropertyType::Currency,
            PropertyValue::FloatingTime(_) => PropertyType::FloatingTime,
            PropertyValue::ErrorCode(_) => PropertyType::ErrorCode,
            PropertyValue::Boolean(_) => PropertyType::Boolean,
            PropertyValue::Integer64(_) => PropertyType::Integer64,
            PropertyValue::String8(_) => PropertyType::String8,
            PropertyValue::Unicode(_) => PropertyType::Unicode,
            PropertyValue::Time(_) => PropertyType::Time,
            PropertyValue::Guid(_) => PropertyType::Guid,
            PropertyValue::Binary(_) => PropertyType::Binary,
            PropertyValue::Object(_) => PropertyType::Object,
            PropertyValue::MultipleInteger16(_) => PropertyType::MultipleInteger16,
            PropertyValue::MultipleInteger32(_) => PropertyType::MultipleInteger32,
            PropertyValue::MultipleFloating32(_) => PropertyType::MultipleFloating32,
            PropertyValue::MultipleFloating64(_) => PropertyType::MultipleFloating64,
            PropertyValue::MultipleCurrency(_) => PropertyType::MultipleCurrency,
            PropertyValue::MultipleFloatingTime(_) => PropertyType::MultipleFloatingTime,
            PropertyValue::MultipleInteger64(_) => PropertyType::MultipleInteger64,
            PropertyValue::MultipleString8(_) => PropertyType::MultipleString8,
            PropertyValue::MultipleUnicode(_) => PropertyType::MultipleUnicode,
            PropertyValue::MultipleTime(_) => PropertyType::MultipleTime,
            PropertyValue::MultipleGuid(_) => PropertyType::MultipleGuid,
            PropertyValue::MultipleBinary(_) => PropertyType::MultipleBinary,
        }
    }
}

fn read_fixed_values<T>(
    f: &mut dyn Read,
    width: usize,
    decode: impl Fn(&[u8]) -> T,
) -> io::Result<Vec<T>> {
    let mut values = Vec::new();
    let mut bytes = vec![0_u8; width];
    loop {
        if f.read(&mut bytes[..1])? == 0 {
            return Ok(values);
        }
        f.read_exact(&mut bytes[1..])?;
        values.push(decode(&bytes));
    }
}

fn require_eof(f: &mut dyn Read) -> io::Result<()> {
    let mut byte = [0_u8; 1];
    if f.read(&mut byte)? == 0 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "count-prefixed property has trailing data",
        ))
    }
}

const MAX_MULTI_VALUE_ELEMENTS: usize = 65_536;
pub(crate) const MAX_PROPERTY_MATERIALIZATION_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct PropertyMaterializationBudget {
    materialized: usize,
}

impl PropertyMaterializationBudget {
    pub(crate) fn new() -> Self {
        Self { materialized: 0 }
    }

    pub(crate) fn charge(&mut self, bytes: usize) -> io::Result<()> {
        self.materialized = self.materialized.checked_add(bytes).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "property materialized-size overflow",
            )
        })?;
        if self.materialized > MAX_PROPERTY_MATERIALIZATION_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "properties exceed the materialization limit",
            ));
        }
        Ok(())
    }
}

fn read_multivalue_count(f: &mut dyn Read) -> io::Result<usize> {
    let count = f.read_u32::<LittleEndian>()? as usize;
    if count > MAX_MULTI_VALUE_ELEMENTS {
        Err(LtpError::InvalidMultiValuePropertyCount(count).into())
    } else {
        Ok(count)
    }
}

impl PropertyValueReadWrite for PropertyValue {
    fn read(f: &mut dyn Read, prop_type: PropertyType) -> io::Result<Self> {
        match prop_type {
            PropertyType::Floating64 => {
                let value = f.read_f64::<LittleEndian>()?;
                Ok(Self::Floating64(value))
            }

            PropertyType::Currency => {
                let value = f.read_i64::<LittleEndian>()?;
                Ok(Self::Currency(value))
            }

            PropertyType::FloatingTime => {
                let value = f.read_f64::<LittleEndian>()?;
                Ok(Self::FloatingTime(value))
            }

            PropertyType::Integer64 => {
                let value = f.read_i64::<LittleEndian>()?;
                Ok(Self::Integer64(value))
            }

            PropertyType::String8 => {
                let mut buffer = Vec::new();
                f.read_to_end(&mut buffer)?;
                if let Some(end) = buffer.iter().position(|&b| b == 0) {
                    buffer.truncate(end);
                }
                Ok(Self::String8(String8Value { buffer }))
            }

            PropertyType::Unicode => {
                let mut buffer = Vec::new();
                while let Ok(ch) = f.read_u16::<LittleEndian>() {
                    if ch == 0 {
                        break;
                    }
                    buffer.push(ch);
                }
                Ok(Self::Unicode(UnicodeValue { buffer }))
            }

            PropertyType::Time => {
                let value = f.read_i64::<LittleEndian>()?;
                Ok(Self::Time(value))
            }

            PropertyType::Guid => {
                let data1 = f.read_u32::<LittleEndian>()?;
                let data2 = f.read_u16::<LittleEndian>()?;
                let data3 = f.read_u16::<LittleEndian>()?;
                let mut data4 = [0; 8];
                f.read_exact(&mut data4)?;
                Ok(Self::Guid(GuidValue {
                    data1,
                    data2,
                    data3,
                    data4,
                }))
            }

            PropertyType::Binary => {
                let mut buffer = Vec::new();
                f.read_to_end(&mut buffer)?;
                Ok(Self::Binary(BinaryValue {
                    buffer: buffer.into(),
                }))
            }

            PropertyType::Object => {
                let node_id = NodeId::read(f)?;
                let size = f.read_u32::<LittleEndian>()?;
                Ok(Self::Object(ObjectValue { node_id, size }))
            }

            PropertyType::MultipleInteger16 => {
                Ok(Self::MultipleInteger16(read_fixed_values(f, 2, |bytes| {
                    i16::from_le_bytes([bytes[0], bytes[1]])
                })?))
            }

            PropertyType::MultipleInteger32 => {
                Ok(Self::MultipleInteger32(read_fixed_values(f, 4, |bytes| {
                    i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                })?))
            }

            PropertyType::MultipleFloating32 => Ok(Self::MultipleFloating32(read_fixed_values(
                f,
                4,
                |bytes| {
                    f32::from_bits(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                },
            )?)),

            PropertyType::MultipleFloating64 => Ok(Self::MultipleFloating64(read_fixed_values(
                f,
                8,
                |bytes| {
                    f64::from_bits(u64::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ]))
                },
            )?)),

            PropertyType::MultipleCurrency => {
                Ok(Self::MultipleCurrency(read_fixed_values(f, 8, |bytes| {
                    i64::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ])
                })?))
            }

            PropertyType::MultipleFloatingTime => Ok(Self::MultipleFloatingTime(
                read_fixed_values(f, 8, |bytes| {
                    f64::from_bits(u64::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ]))
                })?,
            )),

            PropertyType::MultipleInteger64 => {
                Ok(Self::MultipleInteger64(read_fixed_values(f, 8, |bytes| {
                    i64::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ])
                })?))
            }

            PropertyType::MultipleString8 => {
                // ulCount
                let count = read_multivalue_count(f)?;

                // rgulDataOffsets
                let mut offsets = Vec::new();
                for _ in 0..count {
                    offsets.push(f.read_u32::<LittleEndian>()? as usize);
                }
                if offsets.is_empty() {
                    require_eof(f)?;
                }

                // rgDataItems
                let mut start = offsets
                    .len()
                    .checked_add(1)
                    .and_then(|count| count.checked_mul(mem::size_of::<u32>()))
                    .ok_or(LtpError::InvalidMultiValuePropertyOffset(usize::MAX))?;
                let mut values = Vec::new();
                for i in 0..offsets.len() {
                    let next = offsets[i];
                    if next != start {
                        return Err(LtpError::InvalidMultiValuePropertyOffset(next).into());
                    }

                    let mut buffer = if i < offsets.len() - 1 {
                        let next = offsets[i + 1];
                        if next < start {
                            return Err(LtpError::InvalidMultiValuePropertyOffset(next).into());
                        }

                        if next > start {
                            let length = next - start;
                            let mut buffer = Vec::new();
                            f.take(length as u64).read_to_end(&mut buffer)?;
                            if buffer.len() != length {
                                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                            }
                            start = next;
                            buffer
                        } else {
                            Default::default()
                        }
                    } else {
                        let mut buffer = Vec::new();
                        f.read_to_end(&mut buffer)?;
                        buffer
                    };

                    if let Some(end) = buffer.iter().position(|&b| b == 0) {
                        buffer.truncate(end);
                    }

                    values.push(String8Value { buffer });
                }

                Ok(Self::MultipleString8(values))
            }

            PropertyType::MultipleUnicode => {
                // ulCount
                let count = read_multivalue_count(f)?;

                // rgulDataOffsets
                let mut offsets = Vec::new();
                for _ in 0..count {
                    offsets.push(f.read_u32::<LittleEndian>()? as usize);
                }
                if offsets.is_empty() {
                    require_eof(f)?;
                }

                // rgDataItems
                let mut start = offsets
                    .len()
                    .checked_add(1)
                    .and_then(|count| count.checked_mul(mem::size_of::<u32>()))
                    .ok_or(LtpError::InvalidMultiValuePropertyOffset(usize::MAX))?;
                let mut values = Vec::new();
                for i in 0..offsets.len() {
                    let next = offsets[i];
                    if next != start {
                        return Err(LtpError::InvalidMultiValuePropertyOffset(next).into());
                    }

                    let mut buffer = Vec::new();
                    if i < offsets.len() - 1 {
                        let next = offsets[i + 1];
                        if next < start {
                            return Err(LtpError::InvalidMultiValuePropertyOffset(next).into());
                        }
                        if (next - start) % mem::size_of::<u16>() != 0 {
                            return Err(LtpError::InvalidMultiValuePropertyOffset(next).into());
                        }

                        while start < next {
                            let ch = f.read_u16::<LittleEndian>()?;
                            buffer.push(ch);
                            start += mem::size_of::<u16>();
                        }
                    } else {
                        let mut remaining = Vec::new();
                        f.read_to_end(&mut remaining)?;
                        if remaining.len() % mem::size_of::<u16>() != 0 {
                            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                        }
                        for bytes in remaining.chunks_exact(2) {
                            buffer.push(u16::from_le_bytes([bytes[0], bytes[1]]));
                        }
                    };

                    if let Some(end) = buffer.iter().position(|&ch| ch == 0) {
                        buffer.truncate(end);
                    }

                    values.push(UnicodeValue { buffer });
                }

                Ok(Self::MultipleUnicode(values))
            }

            PropertyType::MultipleTime => {
                Ok(Self::MultipleTime(read_fixed_values(f, 8, |bytes| {
                    i64::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ])
                })?))
            }

            PropertyType::MultipleGuid => {
                Ok(Self::MultipleGuid(read_fixed_values(f, 16, |bytes| {
                    GuidValue {
                        data1: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                        data2: u16::from_le_bytes([bytes[4], bytes[5]]),
                        data3: u16::from_le_bytes([bytes[6], bytes[7]]),
                        data4: [
                            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13],
                            bytes[14], bytes[15],
                        ],
                    }
                })?))
            }

            PropertyType::MultipleBinary => {
                // ulCount
                let count = read_multivalue_count(f)?;

                // rgulDataOffsets
                let mut offsets = Vec::new();
                for _ in 0..count {
                    offsets.push(f.read_u32::<LittleEndian>()? as usize);
                }
                if offsets.is_empty() {
                    require_eof(f)?;
                }

                // rgDataItems
                let mut start = offsets
                    .len()
                    .checked_add(1)
                    .and_then(|count| count.checked_mul(mem::size_of::<u32>()))
                    .ok_or(LtpError::InvalidMultiValuePropertyOffset(usize::MAX))?;
                let mut values = Vec::new();
                for i in 0..offsets.len() {
                    let next = offsets[i];
                    if next != start {
                        return Err(LtpError::InvalidMultiValuePropertyOffset(next).into());
                    }

                    let buffer = if i < offsets.len() - 1 {
                        let next = offsets[i + 1];
                        if next < start {
                            return Err(LtpError::InvalidMultiValuePropertyOffset(next).into());
                        }

                        if next > start {
                            let length = next - start;
                            let mut buffer = Vec::new();
                            f.take(length as u64).read_to_end(&mut buffer)?;
                            if buffer.len() != length {
                                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                            }
                            start = next;
                            buffer
                        } else {
                            Default::default()
                        }
                    } else {
                        let mut buffer = Vec::new();
                        f.read_to_end(&mut buffer)?;
                        buffer
                    };

                    values.push(BinaryValue {
                        buffer: buffer.into(),
                    });
                }

                Ok(Self::MultipleBinary(values))
            }

            _ => Err(LtpError::InvalidVariableLengthPropertyType(prop_type).into()),
        }
    }

    fn write(&self, f: &mut dyn Write) -> io::Result<()> {
        match self {
            Self::Floating64(value) => f.write_f64::<LittleEndian>(*value),

            Self::Currency(value) => f.write_i64::<LittleEndian>(*value),

            Self::FloatingTime(value) => f.write_f64::<LittleEndian>(*value),

            Self::Integer64(value) => f.write_i64::<LittleEndian>(*value),

            Self::String8(value) => f.write_all(value.buffer()),

            Self::Unicode(value) => {
                for ch in value.buffer() {
                    f.write_u16::<LittleEndian>(*ch)?;
                }
                Ok(())
            }

            Self::Time(value) => f.write_i64::<LittleEndian>(*value),

            Self::Guid(value) => {
                f.write_u32::<LittleEndian>(value.data1)?;
                f.write_u16::<LittleEndian>(value.data2)?;
                f.write_u16::<LittleEndian>(value.data3)?;
                f.write_all(&value.data4)
            }

            Self::Binary(value) => f.write_all(value.buffer()),

            Self::Object(value) => {
                value.node_id.write(f)?;
                f.write_u32::<LittleEndian>(value.size)
            }

            Self::MultipleInteger16(values) => {
                for value in values {
                    f.write_i16::<LittleEndian>(*value)?;
                }
                Ok(())
            }

            Self::MultipleInteger32(values) => {
                for value in values {
                    f.write_i32::<LittleEndian>(*value)?;
                }
                Ok(())
            }

            Self::MultipleFloating32(values) => {
                for value in values {
                    f.write_f32::<LittleEndian>(*value)?;
                }
                Ok(())
            }

            Self::MultipleFloating64(values) => {
                for value in values {
                    f.write_f64::<LittleEndian>(*value)?;
                }
                Ok(())
            }

            Self::MultipleCurrency(values) => {
                for value in values {
                    f.write_i64::<LittleEndian>(*value)?;
                }
                Ok(())
            }

            Self::MultipleFloatingTime(values) => {
                for value in values {
                    f.write_f64::<LittleEndian>(*value)?;
                }
                Ok(())
            }

            Self::MultipleInteger64(values) => {
                for value in values {
                    f.write_i64::<LittleEndian>(*value)?;
                }
                Ok(())
            }

            Self::MultipleString8(values) => {
                // ulCount
                let count = u32::try_from(values.len())
                    .map_err(|_| LtpError::InvalidMultiValuePropertyCount(values.len()))?;
                f.write_u32::<LittleEndian>(count)?;

                // rgulDataOffsets
                let mut start = (values.len() + 1) * mem::size_of::<u32>();
                for value in values {
                    let offset = u32::try_from(start)
                        .map_err(|_| LtpError::InvalidMultiValuePropertyOffset(start))?;
                    f.write_u32::<LittleEndian>(offset)?;
                    start += value.buffer().len();
                }

                // rgDataItems
                for value in values {
                    f.write_all(value.buffer())?;
                }

                Ok(())
            }

            Self::MultipleUnicode(values) => {
                // ulCount
                let count = u32::try_from(values.len())
                    .map_err(|_| LtpError::InvalidMultiValuePropertyCount(values.len()))?;
                f.write_u32::<LittleEndian>(count)?;

                // rgulDataOffsets
                let mut start = (values.len() + 1) * mem::size_of::<u32>();
                for value in values {
                    let offset = u32::try_from(start)
                        .map_err(|_| LtpError::InvalidMultiValuePropertyOffset(start))?;
                    f.write_u32::<LittleEndian>(offset)?;
                    start += mem::size_of_val(value.buffer());
                }

                // rgDataItems
                for value in values {
                    for ch in value.buffer() {
                        f.write_u16::<LittleEndian>(*ch)?;
                    }
                }

                Ok(())
            }

            Self::MultipleTime(values) => {
                for value in values {
                    f.write_i64::<LittleEndian>(*value)?;
                }
                Ok(())
            }

            Self::MultipleGuid(values) => {
                for value in values {
                    f.write_u32::<LittleEndian>(value.data1)?;
                    f.write_u16::<LittleEndian>(value.data2)?;
                    f.write_u16::<LittleEndian>(value.data3)?;
                    f.write_all(&value.data4)?;
                }
                Ok(())
            }

            Self::MultipleBinary(values) => {
                // ulCount
                let count = u32::try_from(values.len())
                    .map_err(|_| LtpError::InvalidMultiValuePropertyCount(values.len()))?;
                f.write_u32::<LittleEndian>(count)?;

                // rgulDataOffsets
                let mut start = (values.len() + 1) * mem::size_of::<u32>();
                for value in values {
                    let offset = u32::try_from(start)
                        .map_err(|_| LtpError::InvalidMultiValuePropertyOffset(start))?;
                    f.write_u32::<LittleEndian>(offset)?;
                    start += value.buffer().len();
                }

                // rgDataItems
                for value in values {
                    f.write_all(value.buffer())?;
                }

                Ok(())
            }

            _ => Err(LtpError::InvalidVariableLengthPropertyType(self.into()).into()),
        }
    }
}

pub type PropertyTree = dyn HeapTree<Key = PropertyTreeRecordKey, Value = PropertyTreeRecordValue>;

pub trait PropertyContext {
    fn tree(&self) -> &PropertyTree;
    fn properties(&self) -> io::Result<BTreeMap<PropertyTreeRecordKey, PropertyTreeRecordValue>>;
}

struct PropertyContextInner<Pst>
where
    Pst: PstFile,
{
    node: <Pst as PstFile>::NodeBTreeEntry,
    tree: <Pst as PstFile>::PropertyTree,
    block_cache: RefCell<DataBlockCache<Pst>>,
}

impl<Pst> PropertyContextInner<Pst>
where
    Pst: PstFile,
    <Pst as PstFile>::BlockId: BlockId<Index = <Pst as PstFile>::BTreeKey> + BlockIdReadWrite,
    <Pst as PstFile>::ByteIndex: ByteIndexReadWrite,
    <Pst as PstFile>::BlockRef: BlockRefReadWrite,
    <Pst as PstFile>::PageTrailer: PageTrailerReadWrite,
    <Pst as PstFile>::BTreeKey: BTreePageKeyReadWrite,
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
    <Pst as PstFile>::DataTreeBlock: IntermediateTreeBlockReadWrite,
    <<Pst as PstFile>::DataTreeBlock as IntermediateTreeBlock>::Entry:
        IntermediateTreeEntryReadWrite,
    <Pst as PstFile>::DataBlock: BlockReadWrite + Clone,
    <Pst as PstFile>::SubNodeTreeBlockHeader: IntermediateTreeHeaderReadWrite,
    <Pst as PstFile>::SubNodeTreeBlock: IntermediateTreeBlockReadWrite,
    <<Pst as PstFile>::SubNodeTreeBlock as IntermediateTreeBlock>::Entry:
        IntermediateTreeEntryReadWrite,
    <Pst as PstFile>::SubNodeBlock: IntermediateTreeBlockReadWrite,
    <<Pst as PstFile>::SubNodeBlock as IntermediateTreeBlock>::Entry:
        IntermediateTreeEntryReadWrite,
{
    fn new(node: <Pst as PstFile>::NodeBTreeEntry, tree: <Pst as PstFile>::PropertyTree) -> Self {
        Self {
            node,
            tree,
            block_cache: Default::default(),
        }
    }

    fn properties(&self) -> io::Result<BTreeMap<PropertyTreeRecordKey, PropertyTreeRecordValue>> {
        Ok(self
            .tree
            .entries()?
            .into_iter()
            .map(|entry| {
                (
                    entry.key(),
                    PropertyTreeRecordValue::new(entry.data().prop_type(), entry.data().value()),
                )
            })
            .collect())
    }

    fn read_property<R: PstReader>(
        &self,
        f: &mut R,
        encoding: NdbCryptMethod,
        block_btree: &PstFileReadWriteBlockBTree<Pst>,
        page_cache: &mut RootBTreePageCache<<Pst as PstFile>::BlockBTree>,
        value: PropertyTreeRecordValue,
        mut budget: Option<&mut PropertyMaterializationBudget>,
    ) -> io::Result<PropertyValue> {
        let mut reserved = 0_usize;
        let decoded = match value.value() {
            PropertyValueRecord::Heap(heap_id) => {
                if u32::from(heap_id) == 0 {
                    return Ok(PropertyValue::Null);
                }

                let data = self.tree.heap().find_entry(heap_id)?;
                reserved = data.len();
                if let Some(budget) = budget.as_deref_mut() {
                    budget.charge(reserved)?;
                }
                let mut cursor = Cursor::new(data);
                PropertyValueReadWrite::read(&mut cursor, value.prop_type())
            }
            PropertyValueRecord::Node(sub_node_id) => {
                let sub_node =
                    self.node
                        .sub_node()
                        .ok_or(LtpError::PropertySubNodeValueNotFound(u32::from(
                            sub_node_id,
                        )))?;
                let block = block_btree.find_entry(f, sub_node.search_key(), page_cache)?;
                let sub_node_tree = SubNodeTree::<Pst>::read(f, &block)?;
                let block = sub_node_tree.find_entry(f, block_btree, sub_node_id, page_cache)?;
                let block = block_btree.find_entry(f, block.search_key(), page_cache)?;
                let mut block_cache = self.block_cache.borrow_mut();
                let data_tree = match block_cache.remove(&block.block().block()) {
                    Some(data_tree) => data_tree,
                    None => DataTree::read(f, encoding, &block)?,
                };
                reserved = data_tree.declared_size();
                if let Some(budget) = budget.as_deref_mut() {
                    budget.charge(reserved)?;
                }
                let mut data = vec![];
                let result = data_tree
                    .reader(f, encoding, block_btree, page_cache, &mut block_cache)
                    .and_then(|mut r| r.read_to_end(&mut data));
                block_cache.insert(block.block().block(), data_tree);
                let _ = result?;
                let mut cursor = Cursor::new(data);
                PropertyValueReadWrite::read(&mut cursor, value.prop_type())
            }
            small => small
                .small_value(value.prop_type())
                .ok_or(LtpError::InvalidSmallPropertyType(value.prop_type()).into()),
        }?;
        if let Some(budget) = budget {
            let actual = decoded.materialized_size().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "property materialized-size overflow",
                )
            })?;
            if actual > reserved {
                budget.charge(actual - reserved)?;
            }
        }
        Ok(decoded)
    }

    fn stream_property_identity<R: PstReader>(
        &self,
        f: &mut R,
        encoding: NdbCryptMethod,
        block_btree: &PstFileReadWriteBlockBTree<Pst>,
        page_cache: &mut RootBTreePageCache<<Pst as PstFile>::BlockBTree>,
        value: PropertyTreeRecordValue,
    ) -> io::Result<(u16, u64, [u8; 32])> {
        let PropertyValueRecord::Node(sub_node_id) = value.value() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "streamed property is not stored in a subnode",
            ));
        };
        let sub_node = self
            .node
            .sub_node()
            .ok_or(LtpError::PropertySubNodeValueNotFound(u32::from(
                sub_node_id,
            )))?;
        let block = block_btree.find_entry(f, sub_node.search_key(), page_cache)?;
        let sub_node_tree = SubNodeTree::<Pst>::read(f, &block)?;
        let block = sub_node_tree.find_entry(f, block_btree, sub_node_id, page_cache)?;
        let block = block_btree.find_entry(f, block.search_key(), page_cache)?;
        let data_tree = DataTree::read(f, encoding, &block)?;
        let mut block_cache = self.block_cache.borrow_mut();
        let mut reader =
            data_tree.streaming_reader(f, encoding, block_btree, page_cache, &mut block_cache)?;
        let mut hasher = Sha256::new();
        let mut byte_len = 0_u64;
        let mut buffer = [0_u8; 8192];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            byte_len = byte_len
                .checked_add(u64::try_from(read).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "streamed property length overflow",
                    )
                })?)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "streamed property length overflow",
                    )
                })?;
        }
        Ok((
            u16::from(value.prop_type()),
            byte_len,
            hasher.finalize().into(),
        ))
    }
}

pub struct UnicodePropertyContext {
    inner: PropertyContextInner<UnicodePstFile>,
}

impl UnicodePropertyContext {
    pub fn new(
        node: UnicodeNodeBTreeEntry,
        tree: <UnicodePstFile as PstFile>::PropertyTree,
    ) -> Self {
        <Self as PropertyContextReadWrite<UnicodePstFile>>::new(node, tree)
    }

    pub fn read_property<R: PstReader>(
        &self,
        f: &mut R,
        encoding: NdbCryptMethod,
        block_btree: &UnicodeBlockBTree,
        page_cache: &mut RootBTreePageCache<UnicodeBlockBTree>,
        value: PropertyTreeRecordValue,
    ) -> io::Result<PropertyValue> {
        <Self as PropertyContextReadWrite<UnicodePstFile>>::read_property(
            self,
            f,
            encoding,
            block_btree,
            page_cache,
            value,
            None,
        )
    }
}

impl PropertyContext for UnicodePropertyContext {
    fn tree(&self) -> &PropertyTree {
        &self.inner.tree
    }

    fn properties(&self) -> io::Result<BTreeMap<PropertyTreeRecordKey, PropertyTreeRecordValue>> {
        self.inner.properties()
    }
}

impl PropertyContextReadWrite<UnicodePstFile> for UnicodePropertyContext {
    fn new(node: UnicodeNodeBTreeEntry, tree: <UnicodePstFile as PstFile>::PropertyTree) -> Self {
        let inner = PropertyContextInner::new(node, tree);
        Self { inner }
    }

    fn read_property<R: PstReader>(
        &self,
        f: &mut R,
        encoding: NdbCryptMethod,
        block_btree: &UnicodeBlockBTree,
        page_cache: &mut RootBTreePageCache<UnicodeBlockBTree>,
        value: PropertyTreeRecordValue,
        budget: Option<&mut PropertyMaterializationBudget>,
    ) -> io::Result<PropertyValue> {
        self.inner
            .read_property(f, encoding, block_btree, page_cache, value, budget)
    }

    fn stream_property_identity<R: PstReader>(
        &self,
        f: &mut R,
        encoding: NdbCryptMethod,
        block_btree: &UnicodeBlockBTree,
        page_cache: &mut RootBTreePageCache<UnicodeBlockBTree>,
        value: PropertyTreeRecordValue,
    ) -> io::Result<(u16, u64, [u8; 32])> {
        self.inner
            .stream_property_identity(f, encoding, block_btree, page_cache, value)
    }
}

pub struct AnsiPropertyContext {
    inner: PropertyContextInner<AnsiPstFile>,
}

impl AnsiPropertyContext {
    pub fn new(node: AnsiNodeBTreeEntry, tree: <AnsiPstFile as PstFile>::PropertyTree) -> Self {
        <Self as PropertyContextReadWrite<AnsiPstFile>>::new(node, tree)
    }

    pub fn read_property<R: PstReader>(
        &self,
        f: &mut R,
        encoding: NdbCryptMethod,
        block_btree: &AnsiBlockBTree,
        page_cache: &mut RootBTreePageCache<AnsiBlockBTree>,
        value: PropertyTreeRecordValue,
    ) -> io::Result<PropertyValue> {
        <Self as PropertyContextReadWrite<AnsiPstFile>>::read_property(
            self,
            f,
            encoding,
            block_btree,
            page_cache,
            value,
            None,
        )
    }
}

impl PropertyContext for AnsiPropertyContext {
    fn tree(&self) -> &PropertyTree {
        &self.inner.tree
    }

    fn properties(&self) -> io::Result<BTreeMap<PropertyTreeRecordKey, PropertyTreeRecordValue>> {
        self.inner.properties()
    }
}

impl PropertyContextReadWrite<AnsiPstFile> for AnsiPropertyContext {
    fn new(node: AnsiNodeBTreeEntry, tree: <AnsiPstFile as PstFile>::PropertyTree) -> Self {
        let inner = PropertyContextInner::new(node, tree);
        Self { inner }
    }

    fn read_property<R: PstReader>(
        &self,
        f: &mut R,
        encoding: NdbCryptMethod,
        block_btree: &AnsiBlockBTree,
        page_cache: &mut RootBTreePageCache<AnsiBlockBTree>,
        value: PropertyTreeRecordValue,
        budget: Option<&mut PropertyMaterializationBudget>,
    ) -> io::Result<PropertyValue> {
        self.inner
            .read_property(f, encoding, block_btree, page_cache, value, budget)
    }

    fn stream_property_identity<R: PstReader>(
        &self,
        f: &mut R,
        encoding: NdbCryptMethod,
        block_btree: &AnsiBlockBTree,
        page_cache: &mut RootBTreePageCache<AnsiBlockBTree>,
        value: PropertyTreeRecordValue,
    ) -> io::Result<(u16, u64, [u8; 32])> {
        self.inner
            .stream_property_identity(f, encoding, block_btree, page_cache, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_property_budget_rejects_cumulative_materialization() {
        let mut budget = PropertyMaterializationBudget::new();
        budget
            .charge(MAX_PROPERTY_MATERIALIZATION_BYTES)
            .expect("exact property budget is accepted");
        assert!(budget.charge(1).is_err());
    }

    #[test]
    fn variable_multivalue_rejects_untrusted_counts_without_preallocation() {
        for property_type in [
            PropertyType::MultipleString8,
            PropertyType::MultipleUnicode,
            PropertyType::MultipleBinary,
        ] {
            let mut cursor = Cursor::new(u32::MAX.to_le_bytes());
            assert!(PropertyValue::read(&mut cursor, property_type).is_err());
        }

        let count = u32::try_from(MAX_MULTI_VALUE_ELEMENTS + 1).expect("test count fits u32");
        let mut realizable = Vec::with_capacity(4 + count as usize * 4);
        realizable.extend_from_slice(&count.to_le_bytes());
        realizable.resize(4 + count as usize * 4, 0);
        let mut cursor = Cursor::new(realizable);
        assert!(PropertyValue::read(&mut cursor, PropertyType::MultipleBinary).is_err());
    }

    #[test]
    fn variable_multivalue_rejects_offsets_beyond_available_data() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&12_u32.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        for property_type in [
            PropertyType::MultipleString8,
            PropertyType::MultipleUnicode,
            PropertyType::MultipleBinary,
        ] {
            let mut cursor = Cursor::new(bytes.as_slice());
            assert!(PropertyValue::read(&mut cursor, property_type).is_err());
        }
    }

    #[test]
    fn fixed_multivalue_rejects_partial_trailing_elements() {
        for (property_type, width) in [
            (PropertyType::MultipleInteger16, 2),
            (PropertyType::MultipleInteger32, 4),
            (PropertyType::MultipleFloating32, 4),
            (PropertyType::MultipleFloating64, 8),
            (PropertyType::MultipleCurrency, 8),
            (PropertyType::MultipleFloatingTime, 8),
            (PropertyType::MultipleInteger64, 8),
            (PropertyType::MultipleTime, 8),
            (PropertyType::MultipleGuid, 16),
        ] {
            let mut cursor = Cursor::new(vec![0_u8; width - 1]);
            assert!(PropertyValue::read(&mut cursor, property_type).is_err());
            let mut empty = Cursor::new(Vec::<u8>::new());
            assert!(PropertyValue::read(&mut empty, property_type).is_ok());
        }
    }

    #[test]
    fn count_prefixed_multivalue_rejects_trailing_data() {
        for property_type in [
            PropertyType::MultipleString8,
            PropertyType::MultipleUnicode,
            PropertyType::MultipleBinary,
        ] {
            let mut cursor = Cursor::new(vec![0, 0, 0, 0, 1]);
            assert!(PropertyValue::read(&mut cursor, property_type).is_err());
        }
    }

    #[test]
    fn multiple_guid_uses_packed_fixed_width_storage() -> io::Result<()> {
        let values = PropertyValue::MultipleGuid(vec![
            GuidValue::new(1, 2, 3, [4; 8]),
            GuidValue::new(5, 6, 7, [8; 8]),
        ]);
        let mut encoded = Vec::new();
        values.write(&mut encoded)?;
        assert_eq!(encoded.len(), 32);
        assert_eq!(&encoded[..4], &1_u32.to_le_bytes());
        assert!(matches!(
            PropertyValue::read(&mut Cursor::new(encoded), PropertyType::MultipleGuid)?,
            PropertyValue::MultipleGuid(decoded) if decoded.len() == 2
        ));
        Ok(())
    }
}
