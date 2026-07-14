# PSTForge Recovery Utility

> **Status: Superseded source brief.** This file preserves the initial concept
> for traceability. It is not the implementation specification. The 1.0 goal is
> now recovery into smaller standards-conformant PST files for Synology
> MailPlus, not EML/Maildir export. See [the product specification](PRODUCT_SPEC.md),
> [the numbered roadmap](ROADMAP.md), [the living ExecPlan](../.agent/EXECPLAN.md),
> and [repository agent instructions](../AGENTS.md). Where this outline
> conflicts with those documents, the newer documents are authoritative.

The most important superseded statements below are the export-first command
surface, PST splitting as a non-goal, the original five unversioned milestones,
and the EML-focused first coding task. They remain unchanged below as historical
input rather than active requirements.

## Objective

Build a Linux-native command-line utility for diagnosing damaged Outlook PST files and recovering as much usable data as possible.

Initial priority:

1. Open damaged PST files safely
2. Detect corruption
3. Enumerate recoverable folders and items
4. Recover orphaned or partially damaged items
5. Export recovered content to standard formats
6. Produce a detailed recovery report

Do not modify or repair PST files in place.

## Technology Stack

* Language: Rust
* PST parser: `libpff`
* Rust integration: FFI wrapper around `libpff`
* CLI framework: `clap`
* Logging: `tracing`
* Serialization: `serde` and `serde_json`
* Error handling: `thiserror` and `anyhow`
* Build target: Debian 13 x86_64

## Initial Commands

```text
pstforge info <file.pst>
pstforge verify <file.pst>
pstforge recover <file.pst> --output <directory>
pstforge export <file.pst> --format eml --output <directory>
pstforge report <file.pst> --output report.json
```

## Core Requirements

### Safe File Handling

* Open PST files read-only
* Never overwrite the source
* Refuse output paths that point to the source file
* Support files larger than 4 GB
* Record source size, timestamps, and SHA-256 hash
* Continue processing after recoverable item-level errors

### PST Inspection

Display:

* PST format and version
* ANSI or Unicode format
* encryption or encoding mode
* corruption status
* folder count
* item count
* orphaned item count
* recovered item count
* attachment count
* parsing errors

### Recovery

Attempt recovery in this order:

1. Normal folder-tree traversal
2. Recoverable deleted or disconnected items exposed by `libpff`
3. Orphaned item enumeration
4. Partial extraction of damaged messages
5. Attachment extraction when the parent message is damaged

Each recovered object must include a confidence status:

```text
complete
partial
orphaned
damaged
failed
```

### Export Formats

Phase-one formats:

* Email: `.eml`
* Folder collections: Maildir
* Attachments: original filename where available
* Metadata: JSON
* Contacts: JSON initially
* Calendar items: JSON initially

Preserve:

* subject
* sender
* recipients
* sent and received timestamps
* message body
* HTML body
* attachments
* folder path
* message class
* original node identifier
* raw MAPI properties when available

### Output Structure

```text
recovery-output/
├── manifest.json
├── report.json
├── maildir/
├── eml/
├── attachments/
├── contacts/
├── calendar/
├── orphaned/
├── partial/
└── errors/
```

## Recovery Report

Generate a machine-readable JSON report containing:

```json
{
  "source_file": "",
  "source_sha256": "",
  "source_size": 0,
  "pst_format": "",
  "corrupted": false,
  "folders_found": 0,
  "items_found": 0,
  "items_exported": 0,
  "items_partial": 0,
  "items_orphaned": 0,
  "items_failed": 0,
  "attachments_exported": 0,
  "errors": []
}
```

Also generate a concise human-readable summary.

## Internal Architecture

```text
src/
├── main.rs
├── cli.rs
├── error.rs
├── pst/
│   ├── mod.rs
│   ├── ffi.rs
│   ├── file.rs
│   ├── folder.rs
│   ├── item.rs
│   └── property.rs
├── recovery/
│   ├── mod.rs
│   ├── normal.rs
│   ├── orphan.rs
│   └── partial.rs
├── export/
│   ├── mod.rs
│   ├── eml.rs
│   ├── maildir.rs
│   ├── attachment.rs
│   └── json.rs
└── report/
    ├── mod.rs
    └── model.rs
```

## Canonical Item Model

Create an internal model independent of `libpff`:

```rust
struct RecoveredItem {
    source_node_id: Option<u64>,
    item_type: ItemType,
    message_class: Option<String>,
    folder_path: Vec<String>,
    subject: Option<String>,
    sender: Option<String>,
    recipients: Vec<Recipient>,
    sent_at: Option<DateTime<Utc>>,
    received_at: Option<DateTime<Utc>>,
    body_text: Option<String>,
    body_html: Option<String>,
    attachments: Vec<RecoveredAttachment>,
    raw_properties: Vec<RawProperty>,
    recovery_status: RecoveryStatus,
    errors: Vec<String>,
}
```

Do not expose raw `libpff` pointers outside the PST module.

## Error Handling

* A single damaged message must not stop the recovery
* A damaged attachment must not stop the parent message export
* Log the node ID, folder path, and operation for every error
* Distinguish fatal file errors from recoverable item errors
* Return a nonzero exit code when any item fails
* Return a separate exit code when the entire PST cannot be opened

Suggested exit codes:

```text
0 = completed without errors
1 = completed with partial recovery
2 = invalid arguments
3 = PST could not be opened
4 = output failure
5 = internal error
```

## Testing

Create tests using:

* healthy Unicode PST
* healthy ANSI PST
* truncated PST
* PST with orphaned items
* PST containing large attachments
* PST containing embedded messages
* PST containing malformed HTML or RTF
* PST containing contacts and calendar items

For every test file:

1. Record expected folder and item counts
2. Run recovery
3. Verify exported file count
4. Verify no source-file modification
5. Verify recovery report values
6. Verify repeated runs produce deterministic output

## Development Milestones

### Milestone 1

* Rust project skeleton
* `libpff` detection and linking
* Open PST read-only
* Display basic PST metadata
* Enumerate folders

### Milestone 2

* Enumerate messages and attachments
* Build canonical item model
* Export metadata to JSON
* Add structured logging

### Milestone 3

* Export messages to EML
* Export folders to Maildir
* Extract attachments
* Generate recovery report

### Milestone 4

* Enumerate recovered and orphaned items
* Continue after damaged records
* Classify complete and partial recovery
* Add corrupted PST test corpus

### Milestone 5

* Improve malformed-item recovery
* Add resumable recovery jobs
* Add progress reporting
* Package as a Debian `.deb`

## Non-Goals for Initial Release

Do not implement yet:

* PST creation
* PST-to-PST repair
* in-place modification
* PST splitting
* PST merging
* OST conversion
* graphical interface
* password cracking
* calendar recurrence conversion
* full MAPI fidelity

## Codex Rules

* Never modify source PST files
* Prefer explicit error handling over panics
* Add tests for every parser or export change
* Keep FFI code isolated
* Validate every pointer returned by `libpff`
* Free every `libpff` resource deterministically
* Do not silently discard unknown MAPI properties
* Do not claim an item was recovered unless its export succeeded
* Keep output deterministic and resumable
* Document any unsupported PST feature in the recovery report

## First Coding Task

Implement the project skeleton and the following command:

```bash
pstforge info sample.pst
```

The command must:

1. Open the PST read-only
2. Print format and version information
3. Report whether corruption is detected
4. Enumerate folders
5. Count normal, recovered, and orphaned items
6. Output the same information as JSON when `--json` is supplied
7. Never modify the input file
