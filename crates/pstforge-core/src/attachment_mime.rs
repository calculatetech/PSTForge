use std::collections::BTreeSet;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use quick_xml::{
    XmlVersion,
    events::{BytesStart, Event},
    name::ResolveResult,
    reader::NsReader,
};

const CFB_SIGNATURE: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";
const MAX_CONTAINER_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ZIP_ENTRIES: u16 = 10_000;
const MAX_ZIP_DIRECTORY_BYTES: u32 = 8 * 1024 * 1024;
const MAX_CONTENT_TYPES_BYTES: u64 = 1024 * 1024;
const MAX_CONTENT_TYPE_EVENTS: usize = 20_000;
const MAX_CFB_ENTRIES: usize = 100_000;
const MAX_CFB_MINIFAT_BYTES: u64 = 8 * 1024 * 1024;
const OPC_CONTENT_TYPES_NAMESPACE: &[u8] =
    b"http://schemas.openxmlformats.org/package/2006/content-types";

const DOCX_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const XLSX_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const PPTX_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";

pub(crate) struct BlobRange<R> {
    inner: R,
    base: u64,
    len: u64,
    position: u64,
}

impl<R: Read + Seek> BlobRange<R> {
    pub(crate) fn new(mut inner: R, len: u64) -> io::Result<Self> {
        let base = inner.stream_position()?;
        Ok(Self {
            inner,
            base,
            len,
            position: 0,
        })
    }
}

impl<R: Read> Read for BlobRange<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = self.len.saturating_sub(self.position);
        let buffer_len = u64::try_from(buffer.len())
            .map_err(|_| io::Error::other("read buffer length does not fit u64"))?;
        let maximum = usize::try_from(remaining.min(buffer_len))
            .map_err(|_| io::Error::other("blob range length does not fit memory index"))?;
        let read = self.inner.read(&mut buffer[..maximum])?;
        self.position = self
            .position
            .checked_add(
                u64::try_from(read)
                    .map_err(|_| io::Error::other("read length does not fit u64"))?,
            )
            .ok_or_else(|| io::Error::other("blob range position overflow"))?;
        Ok(read)
    }
}

impl<R: Seek> Seek for BlobRange<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::Current(value) => i128::from(self.position) + i128::from(value),
            SeekFrom::End(value) => i128::from(self.len) + i128::from(value),
        };
        if next < 0 || next > i128::from(self.len) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek outside verified attachment blob",
            ));
        }
        let next = u64::try_from(next)
            .map_err(|_| io::Error::other("blob range seek does not fit u64"))?;
        let absolute = self
            .base
            .checked_add(next)
            .ok_or_else(|| io::Error::other("blob range absolute offset overflow"))?;
        self.inner.seek(SeekFrom::Start(absolute))?;
        self.position = next;
        Ok(next)
    }
}

pub(crate) fn detect<R: Read + Seek>(
    reader: &mut R,
    byte_len: u64,
    filename: Option<&str>,
) -> io::Result<Option<&'static str>> {
    let mut signature = [0_u8; 11];
    let signature_len = u64::try_from(signature.len())
        .map_err(|_| io::Error::other("attachment signature length does not fit u64"))?;
    let expected = usize::try_from(byte_len.min(signature_len))
        .map_err(|_| io::Error::other("attachment signature length overflow"))?;
    reader.read_exact(&mut signature[..expected])?;
    reader.rewind()?;
    let bytes = &signature[..expected];

    if let Some(mime) = flat_signature(bytes) {
        return Ok(Some(mime));
    }
    if zip_signature(bytes) {
        let (detected, extension_hint_allowed) =
            detect_ooxml(reader, byte_len).unwrap_or(("application/zip", true));
        return Ok(Some(
            match filename_extension(filename).filter(|_| extension_hint_allowed) {
                Some(value) if value.eq_ignore_ascii_case("docx") => {
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                }
                Some(value) if value.eq_ignore_ascii_case("xlsx") => {
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                }
                Some(value) if value.eq_ignore_ascii_case("pptx") => {
                    "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                }
                _ => detected,
            },
        ));
    }
    if bytes.starts_with(CFB_SIGNATURE) {
        return Ok(detect_legacy_office(reader, byte_len, filename).unwrap_or(None));
    }
    Ok(None)
}

fn filename_extension(filename: Option<&str>) -> Option<&str> {
    filename
        .and_then(|value| value.rsplit_once('.'))
        .map(|(_, extension)| extension)
        .filter(|extension| !extension.is_empty())
}

pub(crate) fn flat_signature(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF, 0xE0]) && bytes.get(6..11) == Some(b"JFIF\0") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some("image/tiff")
    } else {
        None
    }
}

fn zip_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
}

fn detect_ooxml<R: Read + Seek>(reader: &mut R, byte_len: u64) -> io::Result<(&'static str, bool)> {
    let expected_entries = if byte_len <= MAX_CONTAINER_BYTES {
        bounded_standard_zip(reader, byte_len)?
    } else {
        None
    };
    let Some(expected_entries) = expected_entries else {
        return Ok(("application/zip", true));
    };
    reader.rewind()?;
    let mut archive = match zip::ZipArchive::new(reader) {
        Ok(archive) => archive,
        Err(_) => return Ok(("application/zip", true)),
    };
    if archive.len() != expected_entries {
        return Ok(("application/zip", false));
    }
    let content_types = {
        let mut entry = match archive.by_name("[Content_Types].xml") {
            Ok(entry) => entry,
            Err(_) => return Ok(("application/zip", true)),
        };
        if entry.size() > MAX_CONTENT_TYPES_BYTES {
            return Ok(("application/zip", true));
        }
        let capacity = usize::try_from(entry.size())
            .map_err(|_| io::Error::other("content types length does not fit memory index"))?;
        let mut bytes = Vec::with_capacity(capacity);
        entry
            .by_ref()
            .take(MAX_CONTENT_TYPES_BYTES + 1)
            .read_to_end(&mut bytes)?;
        let actual = u64::try_from(bytes.len())
            .map_err(|_| io::Error::other("content types length does not fit u64"))?;
        if bytes.len() != capacity || actual > MAX_CONTENT_TYPES_BYTES {
            return Ok(("application/zip", true));
        }
        bytes
    };
    let overrides = parse_main_content_types(&content_types)?;
    let mut parts = BTreeSet::new();
    if overrides.iter().any(|(part, _)| !parts.insert(part)) {
        return Ok(("application/zip", false));
    }
    let mut matches = Vec::new();
    if overrides
        .iter()
        .any(|(part, value)| part == "/word/document.xml" && value == DOCX_CONTENT_TYPE)
        && archive.index_for_name("word/document.xml").is_some()
    {
        matches.push("application/vnd.openxmlformats-officedocument.wordprocessingml.document");
    }
    if overrides
        .iter()
        .any(|(part, value)| part == "/xl/workbook.xml" && value == XLSX_CONTENT_TYPE)
        && archive.index_for_name("xl/workbook.xml").is_some()
    {
        matches.push("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
    }
    if overrides
        .iter()
        .any(|(part, value)| part == "/ppt/presentation.xml" && value == PPTX_CONTENT_TYPE)
        && archive.index_for_name("ppt/presentation.xml").is_some()
    {
        matches.push("application/vnd.openxmlformats-officedocument.presentationml.presentation");
    }
    Ok(match matches.len() {
        1 => (matches[0], false),
        0 => ("application/zip", true),
        _ => ("application/zip", false),
    })
}

fn bounded_standard_zip<R: Read + Seek>(
    reader: &mut R,
    byte_len: u64,
) -> io::Result<Option<usize>> {
    const EOCD_MINIMUM: u64 = 22;
    const MAX_COMMENT: u64 = 65_535;
    if byte_len < EOCD_MINIMUM {
        return Ok(None);
    }
    let tail_len = byte_len.min(EOCD_MINIMUM + MAX_COMMENT);
    reader.seek(SeekFrom::End(
        -i64::try_from(tail_len).map_err(|_| io::Error::other("ZIP tail length overflow"))?,
    ))?;
    let capacity = usize::try_from(tail_len)
        .map_err(|_| io::Error::other("ZIP tail length does not fit memory index"))?;
    let mut tail = vec![0_u8; capacity];
    reader.read_exact(&mut tail)?;
    let Some(offset) = tail.windows(4).rposition(|window| window == b"PK\x05\x06") else {
        return Ok(None);
    };
    let eocd = &tail[offset..];
    if eocd.len() < 22 {
        return Ok(None);
    }
    let disk = u16::from_le_bytes([eocd[4], eocd[5]]);
    let directory_disk = u16::from_le_bytes([eocd[6], eocd[7]]);
    let entries_on_disk = u16::from_le_bytes([eocd[8], eocd[9]]);
    let entries = u16::from_le_bytes([eocd[10], eocd[11]]);
    let directory_len = u32::from_le_bytes([eocd[12], eocd[13], eocd[14], eocd[15]]);
    let directory_offset = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]);
    let comment_len = usize::from(u16::from_le_bytes([eocd[20], eocd[21]]));
    let directory_end = u64::from(directory_offset) + u64::from(directory_len);
    let valid = disk == 0
        && directory_disk == 0
        && entries_on_disk == entries
        && entries <= MAX_ZIP_ENTRIES
        && directory_len <= MAX_ZIP_DIRECTORY_BYTES
        && eocd.len() == 22 + comment_len
        && directory_end <= byte_len;
    Ok(valid.then_some(usize::from(entries)))
}

fn parse_main_content_types(bytes: &[u8]) -> io::Result<Vec<(String, String)>> {
    let mut reader = NsReader::from_reader(BufReader::new(bytes));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut values = Vec::new();
    let mut depth = 0_usize;
    let mut valid_root = false;
    for _ in 0..MAX_CONTENT_TYPE_EVENTS {
        match reader.read_resolved_event_into(&mut buffer) {
            Ok((namespace, Event::Start(element))) => {
                let in_opc_namespace = matches!(
                    namespace,
                    ResolveResult::Bound(value)
                        if value.as_ref() == OPC_CONTENT_TYPES_NAMESPACE
                );
                if depth == 0 {
                    if valid_root || !in_opc_namespace || element.local_name().as_ref() != b"Types"
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "OOXML content types root is invalid",
                        ));
                    }
                    valid_root = true;
                } else if depth == 1
                    && in_opc_namespace
                    && element.local_name().as_ref() == b"Override"
                {
                    if let Some(value) = parse_content_type_override(&element)? {
                        values.push(value);
                    }
                }
                depth = depth.saturating_add(1);
            }
            Ok((namespace, Event::Empty(element))) => {
                let in_opc_namespace = matches!(
                    namespace,
                    ResolveResult::Bound(value)
                        if value.as_ref() == OPC_CONTENT_TYPES_NAMESPACE
                );
                if depth == 1 && in_opc_namespace && element.local_name().as_ref() == b"Override" {
                    if let Some(value) = parse_content_type_override(&element)? {
                        values.push(value);
                    }
                }
            }
            Ok((_, Event::End(_))) => depth = depth.saturating_sub(1),
            Ok((_, Event::Text(text))) if depth == 0 && !text.is_empty() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "OOXML content types has text outside the root",
                ));
            }
            Ok((_, Event::DocType(_))) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "DOCTYPE is not accepted in OOXML content types",
                ));
            }
            Ok((_, Event::Eof)) if valid_root && depth == 0 => return Ok(values),
            Ok((_, Event::Eof)) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "OOXML content types root is absent or incomplete",
                ));
            }
            Ok(_) => {}
            Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        }
        buffer.clear();
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "OOXML content types exceeded the event limit",
    ))
}

fn parse_content_type_override(element: &BytesStart<'_>) -> io::Result<Option<(String, String)>> {
    let mut part = None;
    let mut content_type = None;
    for attribute in element.attributes().with_checks(true) {
        let attribute =
            attribute.map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if attribute.key.prefix().is_some() {
            continue;
        }
        match attribute.key.local_name().as_ref() {
            b"PartName" => {
                part = Some(
                    attribute
                        .normalized_value(XmlVersion::Implicit1_0)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                        .into_owned(),
                );
            }
            b"ContentType" => {
                content_type = Some(
                    attribute
                        .normalized_value(XmlVersion::Implicit1_0)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                        .into_owned(),
                );
            }
            _ => {}
        }
    }
    Ok(part.zip(content_type))
}

fn cfb_resource_bounds<R: Read + Seek>(reader: &mut R, byte_len: u64) -> io::Result<bool> {
    const CFB_HEADER_BYTES: usize = 512;
    const FREE_SECTOR: u32 = 0xFFFF_FFFF;
    const END_OF_CHAIN: u32 = 0xFFFF_FFFE;

    if byte_len < 512 {
        return Ok(false);
    }
    reader.rewind()?;
    let mut header = [0_u8; CFB_HEADER_BYTES];
    reader.read_exact(&mut header)?;
    if !header.starts_with(CFB_SIGNATURE) || header[28..30] != [0xFE, 0xFF] {
        return Ok(false);
    }
    let major_version = u16::from_le_bytes([header[26], header[27]]);
    let sector_shift = u16::from_le_bytes([header[30], header[31]]);
    let sector_len = match (major_version, sector_shift) {
        (3, 9) => 512_u64,
        (4, 12) => 4096_u64,
        _ => return Ok(false),
    };
    if byte_len % sector_len != 0 {
        return Ok(false);
    }
    let total_sectors = byte_len
        .checked_div(sector_len)
        .and_then(|value| value.checked_sub(1))
        .ok_or_else(|| io::Error::other("CFB sector count underflow"))?;
    let total_sectors_u32 = match u32::try_from(total_sectors) {
        Ok(value) => value,
        Err(_) => return Ok(false),
    };
    let number_of_fat_sectors = header_u32(&header, 44);
    let first_directory_sector = header_u32(&header, 48);
    let first_minifat_sector = header_u32(&header, 60);
    let number_of_minifat_sectors = header_u32(&header, 64);
    let first_difat_sector = header_u32(&header, 68);
    let number_of_difat_sectors = header_u32(&header, 72);
    if number_of_fat_sectors > total_sectors_u32 || number_of_difat_sectors > total_sectors_u32 {
        return Ok(false);
    }

    let fat_sector_count = usize::try_from(number_of_fat_sectors)
        .map_err(|_| io::Error::other("CFB FAT sector count does not fit memory index"))?;
    let minifat_sector_count = usize::try_from(number_of_minifat_sectors)
        .map_err(|_| io::Error::other("CFB MiniFAT sector count does not fit memory index"))?;
    let mut fat_sectors = Vec::with_capacity(fat_sector_count);
    for offset in (76..512).step_by(4) {
        let sector = header_u32(&header, offset);
        if sector != FREE_SECTOR && fat_sectors.len() < fat_sector_count {
            if sector >= total_sectors_u32 {
                return Ok(false);
            }
            fat_sectors.push(sector);
        }
    }
    let entries_per_sector = usize::try_from(sector_len / 4)
        .map_err(|_| io::Error::other("CFB FAT entry count overflow"))?;
    let mut difat_sector = first_difat_sector;
    let mut seen_difat = BTreeSet::new();
    for _ in 0..number_of_difat_sectors {
        if fat_sectors.len() >= fat_sector_count {
            break;
        }
        if difat_sector >= total_sectors_u32 || !seen_difat.insert(difat_sector) {
            return Ok(false);
        }
        let sector = read_cfb_sector(reader, difat_sector, sector_len, total_sectors_u32)?;
        for chunk in sector[..sector.len() - 4].chunks_exact(4) {
            let value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            if value != FREE_SECTOR && fat_sectors.len() < fat_sector_count {
                if value >= total_sectors_u32 {
                    return Ok(false);
                }
                fat_sectors.push(value);
            }
        }
        let end = sector.len() - 4;
        difat_sector = u32::from_le_bytes([
            sector[end],
            sector[end + 1],
            sector[end + 2],
            sector[end + 3],
        ]);
    }
    if fat_sectors.len() != fat_sector_count {
        return Ok(false);
    }

    let directory_entries_per_sector = usize::try_from(sector_len / 128)
        .map_err(|_| io::Error::other("CFB directory entry count overflow"))?;
    let maximum_directory_sectors = MAX_CFB_ENTRIES / directory_entries_per_sector;
    let directory_sectors = cfb_chain_length(
        reader,
        first_directory_sector,
        sector_len,
        total_sectors_u32,
        &fat_sectors,
        entries_per_sector,
        maximum_directory_sectors,
    )?;
    if directory_sectors.is_none() {
        return Ok(false);
    }
    if major_version == 4 && usize::try_from(header_u32(&header, 40)).ok() != directory_sectors {
        return Ok(false);
    }

    let maximum_minifat_sectors = usize::try_from(MAX_CFB_MINIFAT_BYTES / sector_len)
        .map_err(|_| io::Error::other("CFB MiniFAT sector limit overflow"))?;
    if minifat_sector_count > maximum_minifat_sectors {
        return Ok(false);
    }
    let minifat_sectors = if first_minifat_sector == END_OF_CHAIN {
        Some(0)
    } else {
        cfb_chain_length(
            reader,
            first_minifat_sector,
            sector_len,
            total_sectors_u32,
            &fat_sectors,
            entries_per_sector,
            maximum_minifat_sectors,
        )?
    };
    Ok(minifat_sectors.is_some())
}

fn header_u32(header: &[u8; 512], offset: usize) -> u32 {
    u32::from_le_bytes([
        header[offset],
        header[offset + 1],
        header[offset + 2],
        header[offset + 3],
    ])
}

fn read_cfb_sector<R: Read + Seek>(
    reader: &mut R,
    sector: u32,
    sector_len: u64,
    total_sectors: u32,
) -> io::Result<Vec<u8>> {
    if sector >= total_sectors {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "CFB sector is outside the attachment",
        ));
    }
    let offset = u64::from(sector)
        .checked_add(1)
        .and_then(|value| value.checked_mul(sector_len))
        .ok_or_else(|| io::Error::other("CFB sector offset overflow"))?;
    reader.seek(SeekFrom::Start(offset))?;
    let capacity = usize::try_from(sector_len)
        .map_err(|_| io::Error::other("CFB sector length does not fit memory index"))?;
    let mut bytes = vec![0_u8; capacity];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn cfb_chain_length<R: Read + Seek>(
    reader: &mut R,
    first_sector: u32,
    sector_len: u64,
    total_sectors: u32,
    fat_sectors: &[u32],
    entries_per_sector: usize,
    maximum_sectors: usize,
) -> io::Result<Option<usize>> {
    const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
    let mut current = first_sector;
    let mut seen = BTreeSet::new();
    for count in 0..=maximum_sectors {
        if current == END_OF_CHAIN {
            return Ok(Some(count));
        }
        if current >= total_sectors || !seen.insert(current) {
            return Ok(None);
        }
        let index = usize::try_from(current)
            .map_err(|_| io::Error::other("CFB sector index does not fit memory index"))?;
        let fat_sector_index = index / entries_per_sector;
        let Some(&fat_sector) = fat_sectors.get(fat_sector_index) else {
            return Ok(None);
        };
        let entry_index = index % entries_per_sector;
        let entry_offset = u64::try_from(entry_index)
            .ok()
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| io::Error::other("CFB FAT entry offset overflow"))?;
        let offset = u64::from(fat_sector)
            .checked_add(1)
            .and_then(|value| value.checked_mul(sector_len))
            .and_then(|value| value.checked_add(entry_offset))
            .ok_or_else(|| io::Error::other("CFB FAT entry offset overflow"))?;
        reader.seek(SeekFrom::Start(offset))?;
        let mut next = [0_u8; 4];
        reader.read_exact(&mut next)?;
        current = u32::from_le_bytes(next);
    }
    Ok(None)
}

fn detect_legacy_office<R: Read + Seek>(
    reader: &mut R,
    byte_len: u64,
    filename: Option<&str>,
) -> io::Result<Option<&'static str>> {
    if byte_len > MAX_CONTAINER_BYTES || !cfb_resource_bounds(reader, byte_len)? {
        return Ok(None);
    }
    reader.rewind()?;
    let mut compound = match cfb::OpenOptions::new()
        .max_buffer_size(1024 * 1024)
        .open_with(reader)
    {
        Ok(compound) => compound,
        Err(_) => return Ok(None),
    };
    let mut word = None;
    let mut excel = None;
    let mut powerpoint = None;
    let mut current_user = None;
    for (index, entry) in compound.walk().enumerate() {
        if index >= MAX_CFB_ENTRIES {
            return Ok(None);
        }
        if !entry.is_stream() || entry.path().parent() != Some(Path::new("/")) {
            continue;
        }
        if entry.name().eq_ignore_ascii_case("WordDocument") {
            word = Some(entry.path().to_path_buf());
        } else if entry.name().eq_ignore_ascii_case("Workbook")
            || entry.name().eq_ignore_ascii_case("Book")
        {
            if excel.is_some() {
                return Ok(None);
            }
            excel = Some(entry.path().to_path_buf());
        } else if entry.name().eq_ignore_ascii_case("PowerPoint Document") {
            powerpoint = Some(entry.path().to_path_buf());
        } else if entry.name().eq_ignore_ascii_case("Current User") {
            current_user = Some(entry.path().to_path_buf());
        }
    }
    Ok(match (&word, &excel, &powerpoint) {
        (Some(path), None, None) if valid_doc_stream(&mut compound, path)? => {
            Some("application/msword")
        }
        (None, Some(path), None) if valid_xls_stream(&mut compound, path)? => {
            Some("application/vnd.ms-excel")
        }
        (None, None, Some(path))
            if valid_ppt_stream(&mut compound, path)?
                && match &current_user {
                    Some(current_user) => valid_current_user_stream(&mut compound, current_user)?,
                    None => false,
                } =>
        {
            Some("application/vnd.ms-powerpoint")
        }
        (Some(_), None, None)
            if filename_extension(filename)
                .is_some_and(|value| value.eq_ignore_ascii_case("doc")) =>
        {
            Some("application/msword")
        }
        (None, Some(_), None)
            if filename_extension(filename)
                .is_some_and(|value| value.eq_ignore_ascii_case("xls")) =>
        {
            Some("application/vnd.ms-excel")
        }
        (None, None, Some(_))
            if filename_extension(filename)
                .is_some_and(|value| value.eq_ignore_ascii_case("ppt")) =>
        {
            Some("application/vnd.ms-powerpoint")
        }
        _ => None,
    })
}

fn stream_prefix<F: Read + Seek>(
    compound: &mut cfb::CompoundFile<F>,
    path: &Path,
    length: usize,
) -> io::Result<Vec<u8>> {
    let mut stream = compound.open_stream(path)?;
    let length_u64 = u64::try_from(length)
        .map_err(|_| io::Error::other("legacy Office prefix length overflow"))?;
    if stream.len() < length_u64 {
        return Ok(Vec::new());
    }
    let mut bytes = vec![0_u8; length];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn valid_doc_stream<F: Read + Seek>(
    compound: &mut cfb::CompoundFile<F>,
    path: &Path,
) -> io::Result<bool> {
    Ok(stream_prefix(compound, path, 2)? == b"\xEC\xA5")
}

fn valid_xls_stream<F: Read + Seek>(
    compound: &mut cfb::CompoundFile<F>,
    path: &Path,
) -> io::Result<bool> {
    let bytes = stream_prefix(compound, path, 8)?;
    if bytes.len() != 8 {
        return Ok(false);
    }
    let record_type = u16::from_le_bytes([bytes[0], bytes[1]]);
    let record_len = u16::from_le_bytes([bytes[2], bytes[3]]);
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let substream = u16::from_le_bytes([bytes[6], bytes[7]]);
    Ok(record_type == 0x0809
        && record_len >= 4
        && matches!(version, 0x0500 | 0x0600)
        && substream == 0x0005)
}

fn valid_ppt_stream<F: Read + Seek>(
    compound: &mut cfb::CompoundFile<F>,
    path: &Path,
) -> io::Result<bool> {
    let bytes = stream_prefix(compound, path, 8)?;
    if bytes.len() != 8 {
        return Ok(false);
    }
    let version_instance = u16::from_le_bytes([bytes[0], bytes[1]]);
    let record_type = u16::from_le_bytes([bytes[2], bytes[3]]);
    let record_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let stream_len = compound.entry(path)?.len();
    Ok(version_instance & 0x000F == 0x000F
        && record_type == 0x03E8
        && u64::from(record_len) <= stream_len.saturating_sub(8))
}

fn valid_current_user_stream<F: Read + Seek>(
    compound: &mut cfb::CompoundFile<F>,
    path: &Path,
) -> io::Result<bool> {
    let bytes = stream_prefix(compound, path, 4)?;
    Ok(bytes.len() == 4 && u16::from_le_bytes([bytes[2], bytes[3]]) == 0x0FF6)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Seek, Write};

    use super::{BlobRange, detect};

    fn zip_with(content_type: &str, main_part: &str) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("[Content_Types].xml", options).unwrap();
        write!(
            writer,
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/{main_part}" ContentType="{content_type}"/></Types>"#
        )
        .unwrap();
        writer.start_file(main_part, options).unwrap();
        writer.write_all(b"payload").unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn zip_with_raw_content_types(xml: &[u8], duplicate: bool) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("[Content_Types].xml", options).unwrap();
        writer.write_all(xml).unwrap();
        if duplicate {
            writer.start_file("[Content_Types].xm_", options).unwrap();
            writer.write_all(xml).unwrap();
        }
        writer.start_file("word/document.xml", options).unwrap();
        writer.write_all(b"payload").unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();
        if duplicate {
            let placeholder = b"[Content_Types].xm_";
            for index in 0..=bytes.len().saturating_sub(placeholder.len()) {
                if bytes[index..].starts_with(placeholder) {
                    bytes[index + placeholder.len() - 1] = b'l';
                }
            }
        }
        bytes
    }

    fn conflicting_ooxml() -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("[Content_Types].xml", options).unwrap();
        write!(
            writer,
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="{}"/><Override PartName="/xl/workbook.xml" ContentType="{}"/></Types>"#,
            super::DOCX_CONTENT_TYPE,
            super::XLSX_CONTENT_TYPE,
        )
        .unwrap();
        for name in ["word/document.xml", "xl/workbook.xml"] {
            writer.start_file(name, options).unwrap();
            writer.write_all(b"payload").unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn cfb_with_streams(names: &[&str]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut compound = cfb::CompoundFile::create(cursor).unwrap();
        for name in names {
            let payload: &[u8] = match *name {
                "WordDocument" => b"\xEC\xA5",
                "Workbook" | "Book" => b"\x09\x08\x04\x00\x00\x06\x05\x00",
                "PowerPoint Document" => b"\x0F\x00\xE8\x03\x00\x00\x00\x00",
                "Current User" => b"\x00\x00\xF6\x0F",
                _ => b"payload",
            };
            compound
                .create_stream(format!("/{name}"))
                .unwrap()
                .write_all(payload)
                .unwrap();
        }
        compound.into_inner().into_inner()
    }

    fn cfb_with_nested_word_stream() -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut compound = cfb::CompoundFile::create(cursor).unwrap();
        compound.create_storage("/embedded").unwrap();
        compound
            .create_stream("/embedded/WordDocument")
            .unwrap()
            .write_all(b"payload")
            .unwrap();
        compound.into_inner().into_inner()
    }

    #[test]
    fn detects_common_zip_and_office_families() {
        let cases = [
            (
                zip_with(super::DOCX_CONTENT_TYPE, "word/document.xml"),
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            (
                zip_with(super::XLSX_CONTENT_TYPE, "xl/workbook.xml"),
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            (
                zip_with(super::PPTX_CONTENT_TYPE, "ppt/presentation.xml"),
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            ),
            (cfb_with_streams(&["WordDocument"]), "application/msword"),
            (cfb_with_streams(&["Workbook"]), "application/vnd.ms-excel"),
            (
                cfb_with_streams(&["PowerPoint Document", "Current User"]),
                "application/vnd.ms-powerpoint",
            ),
        ];
        for (bytes, expected) in cases {
            assert_eq!(
                detect(&mut Cursor::new(&bytes), bytes.len() as u64, None).unwrap(),
                Some(expected)
            );
        }
    }

    #[test]
    fn ambiguous_and_damaged_containers_do_not_claim_an_office_subtype() {
        let generic = zip_with("application/octet-stream", "word/document.xml");
        assert_eq!(
            detect(&mut Cursor::new(&generic), generic.len() as u64, None).unwrap(),
            Some("application/zip")
        );
        let mismatched = zip_with(super::DOCX_CONTENT_TYPE, "xl/workbook.xml");
        assert_eq!(
            detect(&mut Cursor::new(&mismatched), mismatched.len() as u64, None).unwrap(),
            Some("application/zip")
        );
        let spoofed = zip_with_raw_content_types(
            br#"<evil:Types xmlns:evil="urn:not-opc"><evil:Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></evil:Types>"#,
            false,
        );
        assert_eq!(
            detect(&mut Cursor::new(&spoofed), spoofed.len() as u64, None).unwrap(),
            Some("application/zip")
        );
        let duplicate = zip_with_raw_content_types(
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
            true,
        );
        assert_eq!(
            detect(&mut Cursor::new(&duplicate), duplicate.len() as u64, None).unwrap(),
            Some("application/zip")
        );
        let conflicting_ooxml = conflicting_ooxml();
        assert_eq!(
            detect(
                &mut Cursor::new(&conflicting_ooxml),
                conflicting_ooxml.len() as u64,
                Some("misleading.docx"),
            )
            .unwrap(),
            Some("application/zip")
        );
        let conflicting = cfb_with_streams(&["WordDocument", "Workbook"]);
        assert_eq!(
            detect(
                &mut Cursor::new(&conflicting),
                conflicting.len() as u64,
                None,
            )
            .unwrap(),
            None
        );
        let nested = cfb_with_nested_word_stream();
        assert_eq!(
            detect(&mut Cursor::new(&nested), nested.len() as u64, None).unwrap(),
            None
        );
        let damaged_zip = b"PK\x03\x04damaged";
        assert_eq!(
            detect(
                &mut Cursor::new(damaged_zip),
                damaged_zip.len() as u64,
                None,
            )
            .unwrap(),
            Some("application/zip")
        );
        let damaged_cfb = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1damaged";
        assert_eq!(
            detect(
                &mut Cursor::new(damaged_cfb),
                damaged_cfb.len() as u64,
                None,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn source_extension_is_only_a_container_correlated_hint() {
        let generic_zip = zip_with("application/octet-stream", "payload.bin");
        assert_eq!(
            detect(
                &mut Cursor::new(&generic_zip),
                generic_zip.len() as u64,
                Some("RECOVERED.DOCX"),
            )
            .unwrap(),
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        );
        let invalid_word = {
            let cursor = Cursor::new(Vec::new());
            let mut compound = cfb::CompoundFile::create(cursor).unwrap();
            compound
                .create_stream("/WordDocument")
                .unwrap()
                .write_all(b"damaged")
                .unwrap();
            compound.into_inner().into_inner()
        };
        assert_eq!(
            detect(
                &mut Cursor::new(&invalid_word),
                invalid_word.len() as u64,
                None,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            detect(
                &mut Cursor::new(&invalid_word),
                invalid_word.len() as u64,
                Some("recovered.doc"),
            )
            .unwrap(),
            Some("application/msword")
        );
        let arbitrary = b"not a container";
        assert_eq!(
            detect(
                &mut Cursor::new(arbitrary),
                arbitrary.len() as u64,
                Some("renamed.docx"),
            )
            .unwrap(),
            None
        );
        assert_eq!(
            detect(
                &mut Cursor::new(&invalid_word),
                invalid_word.len() as u64,
                Some("conflict.xls"),
            )
            .unwrap(),
            None
        );
        let proven_xlsx = zip_with(super::XLSX_CONTENT_TYPE, "xl/workbook.xml");
        assert_eq!(
            detect(
                &mut Cursor::new(&proven_xlsx),
                proven_xlsx.len() as u64,
                Some("misnamed.docx"),
            )
            .unwrap(),
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        );
    }

    #[test]
    fn cfb_chain_limit_stops_before_container_parser_allocation() {
        let mut bytes = vec![0_u8; 5 * 512];
        let fat = &mut bytes[512..1024];
        fat[4..8].copy_from_slice(&2_u32.to_le_bytes());
        fat[8..12].copy_from_slice(&3_u32.to_le_bytes());
        fat[12..16].copy_from_slice(&0xFFFF_FFFE_u32.to_le_bytes());
        assert_eq!(
            super::cfb_chain_length(&mut Cursor::new(bytes), 1, 512, 4, &[0], 128, 2,).unwrap(),
            None
        );
    }

    #[test]
    fn blob_range_cannot_read_or_seek_into_an_adjacent_payload() {
        let mut cursor = Cursor::new(b"prefixPAYLOADadjacent".to_vec());
        cursor.set_position(6);
        let mut range = BlobRange::new(cursor, 7).unwrap();
        let mut output = Vec::new();
        range.read_to_end(&mut output).unwrap();
        assert_eq!(output, b"PAYLOAD");
        assert!(range.seek(std::io::SeekFrom::Start(8)).is_err());
    }
}
