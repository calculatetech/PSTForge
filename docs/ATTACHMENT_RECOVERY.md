# Attachment Recovery And Type Detection

PSTForge preserves the complete attachment byte stream whenever `libpff` can
recover it. A missing or damaged filename or MIME property does not make the
payload disposable. Type detection supplies import metadata; it never rewrites,
converts, repairs, or discards the original bytes.

## Recovery Confidence

1. **Source MIME:** A nonempty source MIME type is preserved unchanged.
   PSTForge does not replace it with a detector result.
2. **Proven subtype:** A missing MIME type is derived only when a complete
   payload has an exact format signature or a bounded container parse proves
   one unambiguous subtype.
3. **Correlated source hint:** A recognized source filename extension can
   refine an otherwise generic container when both independent facts agree:
   ZIP plus `.docx`, `.xlsx`, or `.pptx`; or CFB plus the matching root Office
   stream and `.doc`, `.xls`, or `.ppt`. A proven subtype overrides a
   conflicting extension, and conflicting container families are not resolved
   by the filename.
4. **Container only:** A ZIP payload that cannot be proven to be one supported
   Office Open XML subtype is labeled `application/zip`. This includes damaged,
   encrypted, unsupported, and ambiguous ZIP packages.
5. **Unknown:** Other unrecognized payloads remain without a MIME type. When
   the source also has no nonempty filename, PSTForge assigns a deterministic
   `Recovered attachment {index}.bin` name so the exact bytes remain available
   to later recovery and forensic tools.

When the source filename is absent, PSTForge generates
`Recovered attachment {index}.{extension}` from the strongest available type
evidence. A payload-proven type takes precedence over a conflicting source MIME
value for this generated display name, while the original MIME property itself
remains unchanged. A recognized source MIME value supplies the extension when
payload detection is inconclusive. Supported generated extensions are `.pdf`,
`.png`, `.jpg`, `.gif`, `.tif`, `.zip`, `.docx`, `.xlsx`, `.pptx`, `.doc`,
`.xls`, and `.ppt`. Other by-value payloads receive `.bin`; embedded Message
objects receive `.msg`. PSTForge never changes a nonempty source filename.

The filename and MIME type are separate MAPI properties:
[PidTagAttachFilename](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtagattachfilename-canonical-property),
[PidTagAttachLongFilename](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtagattachlongfilename-canonical-property),
and
[PidTagAttachMimeTag](https://learn.microsoft.com/en-us/office/client-developer/outlook/mapi/pidtagattachmimetag-canonical-property).
A filename is therefore independent source evidence, but it is not sufficient
by itself. Arbitrary bytes named `.docx` are not classified as Word.

## Safely Detected Formats

- PDF: exact `%PDF-` header
- PNG: complete PNG signature
- JPEG: complete JFIF APP0 identifier
- GIF: `GIF87a` or `GIF89a`
- TIFF: classic little-endian or big-endian TIFF header
- ZIP: ZIP local-file, empty-archive, or spanning signature
- DOCX: readable ZIP package with `[Content_Types].xml`, the Word main-document
  content type, and `word/document.xml`
- XLSX: readable ZIP package with `[Content_Types].xml`, the SpreadsheetML
  workbook content type, and `xl/workbook.xml`
- PPTX: readable ZIP package with `[Content_Types].xml`, the PresentationML
  presentation content type, and `ppt/presentation.xml`
- DOC: readable Compound File Binary container with an unambiguous
  `WordDocument` stream and valid FIB identifier, or that container/stream plus
  a source `.doc` filename when corruption prevents marker validation
- XLS: readable Compound File Binary container with an unambiguous `Workbook`
  or legacy `Book` stream and valid workbook BOF, or that container/stream plus
  a source `.xls` filename
- PPT: readable Compound File Binary container with an unambiguous
  `PowerPoint Document` and `Current User` record structure, or the main
  container/stream plus a source `.ppt` filename

The Office checks establish a document family, not that every internal record
is intact or that Microsoft Office can render the document. PSTForge does not
decrypt encrypted attachments, repair their internal structures, or infer
macro-enabled and template variants before those variants have their own
focused evidence.

## Corrupt And Ambiguous Sources

Detection is deliberately bounded. PSTForge reads only the attachment's
verified blob range, caps archive entry counts and XML expansion, and does not
extract document content to classify it. Parser failure is a metadata failure,
not an attachment failure.

Structural container inspection is capped at 256 MiB per attachment. That is
above Exchange Online's documented configurable 150 MB maximum message size
and the 25 MB personal Gmail attachment limit, while leaving a fixed resource
ceiling for hostile or badly damaged input. Larger attachments still retain
their exact bytes and signature-level classification; PSTForge merely declines
to parse their internal container for a more specific subtype. See
[Exchange Online limits](https://learn.microsoft.com/en-us/office365/servicedescriptions/exchange-online-service-description/exchange-online-limits)
and [Gmail attachment limits](https://support.google.com/mail/answer/6584).

A damaged ZIP that retains an exact ZIP signature is preserved as
`application/zip`, or receives an Office subtype as a correlated source hint
when a recognized source extension survives. Without that independent hint,
PSTForge does not claim DOCX, XLSX, or PPTX unless both package metadata and
the required main part survive. A Compound File Binary header alone cannot
distinguish DOC, XLS, PPT, MSG, and other OLE-based formats. If the directory
cannot be parsed or its main streams conflict, the payload stays unknown and
is available as `.bin` when no nonempty source filename survives.

This policy favors recoverable bytes and explicit uncertainty over a
convenient but false type label.
