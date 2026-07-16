# pstforge-pst

This MIT crate adapts Microsoft `outlook-pst` 1.2.0 to create new Unicode
version 23 PST files. See `UPSTREAM.md` for the pinned source revision and
`LICENSE` for Microsoft's retained notice.

The PST file format is publicly documented in the [MS-PST](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/141923d5-15ab-4ef1-a524-6dce75aae546) open specification. Data structures and type names generally mimic the concepts and names in that document, with some adjustment for readability and to match Rust language conventions. As much as possible, everything in this crate should have a deep link to the documentation it is based on in the doc comments.

## PSTForge creation support

The `writer` module programmatically creates a compact store with fresh header,
allocation maps, BBT, NBT, property contexts, table contexts, required folders,
and a plain-text message. It does not load or embed a template PST. Existing
store modification remains intentionally unsupported; PSTForge only builds new
output stores.

## Upstream limitation

The pinned upstream project is suitable for read-only access to PST files and
does not provide general new-object creation. PSTForge's creation path is an
adaptation maintained in this crate.

However, this version does support [Crash Recovery and AMap Rebuilding](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/d9bcc1fd-c66a-41b3-b6d7-ed09d2a25ced), which is a step towards supporting [Transactional Semantics](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/bc5a92df-7fc1-4dc2-9c7c-5677237dd73a) when modifying a PST file. If you plan on implementing PST file modification, you can use this as a reference for those features.

If you choose to modify the PST files, please be careful to follow all of the guidance in the [Maintaining Data Integrity](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/5e1a4d6b-ebbf-4658-9aa7-824929233044) section of the specification to avoid corrupting your PST files in a way that prevents Outlook (or this library) from opening them anymore.
