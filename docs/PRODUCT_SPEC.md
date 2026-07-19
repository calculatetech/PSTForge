# PSTForge 1.0 Product Specification

## Purpose

PSTForge is a Linux command-line recovery utility for operators who have a
large or damaged Outlook Personal Storage Table (PST) that existing tools
cannot process reliably. It reads the source without modification, accounts
for reachable and recoverable PST content, and produces smaller PST files that can be
imported independently into Synology MailPlus Server. Smaller parts provide
restart and import checkpoints and limit the scope of a failed import.

Synology MailPlus is the primary 1.0 consumer. Outlook is a secondary
compatibility check. PSTForge is noninteractive and suitable for unattended
runs, while retaining concise human progress and machine-readable reports.

## Scope

PSTForge 1.0 supports healthy and damaged ANSI and Unicode PST input that
`libpff` can open. It preserves every readable native item and useful property
that a PST reader could consume, including mail, contacts, distribution lists,
appointments, meetings, tasks, notes, documents, unknown message classes,
recipients, attachments, recursively embedded items, named properties,
folder/store metadata, and hidden associated contents. It also recovers
deleted or disconnected items exposed by `libpff`, orphan items, and fragment
candidates in explicit aggressive mode when the input library can prove
fragment origin.

PSTForge 1.0 always writes new 64-bit Unicode PST version 23 files with
512-byte pages. Output uses the format's compressible permutation encoding for
broad compatibility; this encoding is obfuscation, not password protection.
Each part is a complete PST with its own message store identity and required
folder tables.

The following are outside 1.0: editing or repairing the source, PST merging,
OST conversion, password cracking, EML, Maildir, PDF, general attachment
export, GUI work, date-range partitioning, and folder-based partitioning.
Readable native PST content is never excluded merely because MailPlus does not
display its item class. Data that source corruption or the output PST format
prevents PSTForge from preserving is counted and explained.

## Command-Line Interface

The binary is named `pstforge`. Every command accepts `--color
<auto|always|never>`, `--log-format <human|json>`, `--quiet`, and repeatable
`-v` verbosity. Human-readable command results go to stdout. Diagnostics,
progress, and logs go to stderr. `--json` replaces the final stdout result with
one versioned JSON document; it does not change stderr unless
`--log-format json` is also selected. No command prompts interactively.

### `pstforge info <source.pst> [--json]`

`info` performs a quick, read-only inspection. It reports the canonical source
path, size, file type, ANSI or Unicode format, page variant, encryption mode,
`libpff` corruption flag, timestamps, and tool/library versions. It does not
enumerate every item and does not invoke deleted-item recovery.

### `pstforge verify <source.pst> [--mode <full|recovery>] [--json]`

`verify` defaults to `full`. Full mode traverses all reachable folders and mail
and validates every readable property and attachment stream without writing a
recovery job. Recovery mode additionally invokes balanced deleted, recovered,
and orphan discovery. The result states what was checked; it never calls the
`libpff` corruption flag a complete integrity proof.

The full-mode inventory uses a bounded event stream and reports folders,
normal mail, recipients, attachments, embedded messages, unsupported message
classes, raw-property and payload byte totals, the peak stream chunk, and a
capped issue list with an omitted-issue count. Version 0.1.1 emits inspection
schema `1.1.0`; absent recovery scans remain `null`, never zero.

### `pstforge recover <source.pst> --output <job-dir> [--json]`

Version 0.3.0 exposes balanced recovery as an operator-visible checkpoint.
It processes reachable mail first, invokes `libpff` deleted-item recovery with
the balanced flag set, then enumerates recovered and orphan collections. Every
emitted candidate records provenance, source node identifier when available,
recovery index, occurrence, completeness, status, private metadata, and
content-addressed payloads in a bundled-SQLite WAL job under
`<job-dir>/.pstforge`.

This command creates durable canonical recovery input, not importable PST
parts. A fresh invocation refuses a nonempty job directory. It computes the
full source SHA-256 before native parsing while holding a read-only descriptor.
Before completion it rechecks the descriptor and path device, inode, size,
mtime, and ctime; any filesystem-mediated content change updates this identity
even if size and mtime are restored. Exit status `1` means the ledger is usable
but one or more candidates or substreams are partial; output/durable-state
failure is `4`.

### `pstforge split <source.pst> --output <job-dir> [OPTIONS]`

`split` is the primary command. Its stable 1.0 options are:

    --max-pst-size <SIZE>        Default: 4GiB. Accept IEC or SI suffixes.
    --recovery <MODE>            balanced (default) or aggressive.
    --resume                     Resume a matching interrupted job.
    --keep-work                  Retain the private spool after success.
    --json                       Print the final summary as JSON.

`split` uses a durable SQLite ledger and private payload pack so recovery and
writing can resume without rereading committed payloads. This restartability
costs approximately one additional readable-payload write and corresponding
temporary capacity. `--keep-work` retains that private state after successful
completion; otherwise it is removed after finalized parts and accounting are
durable.

Without `--resume`, an existing nonempty job directory is an error. With
`--resume`, the source SHA-256, source identity, job schema, recovery mode,
part-size policy, writer format, and compatible tool major version must match.
A mismatch is refused and neither existing parts nor state are changed. A
fresh job retains the three-times-source conservative capacity requirement.
On resume, validated allocation already consumed by the matching job is
credited against the requirement.

PSTForge owns only ledger-tracked output names. It ignores and preserves
untracked files placed in the public `parts/` directory, including human
validation logs and repaired comparison PSTs, and never credits their
allocation to the job. If an untracked file occupies the preferred
`part-NNNN.pst` name, PSTForge aborts before writing that part with an explicit
name-conflict diagnostic. It never renames, replaces, deletes, or works around
the pre-existing file, so repaired or incompatible data cannot be mistaken for
an official split part.

Balanced recovery processes the normal tree, `libpff` deleted/recovered items,
and orphan items. Aggressive recovery also sets the `libpff` flags to ignore
allocation data and scan fragments. Items from libpff's generic recovered-item
collection retain `recovered` provenance; they are not relabeled as fragments.
Provenance `fragment` is reserved for candidates whose fragment origin is
explicitly exposed by the native boundary. Aggressive non-normal candidates
retain lower confidence and are never presented as ordinary reachable mail.

The maximum is a hard target for a normal part. Packing accounts for complete
on-disk PST overhead, validates the final size, and repacks before publication
if needed. If one indivisible message is itself too large, PSTForge writes it
alone to an oversize part, marks that part and item in reports, and returns
partial-success status rather than discarding content.

### `pstforge report <job-dir> [--json]`

`report` reads only durable job state and finalized part manifests. It does not
reopen the source PST. It recreates the bounded human or JSON summary and
fails if the job state or a finalized part hash is inconsistent.

## Source Safety

PSTForge refuses non-regular files, source symlinks, an output directory that
contains the source, and a source path that aliases any output file. It opens
the source read-only and retains identity metadata for the entire job. Before
recovery it records canonical path, device, inode, byte size, modification
time, and SHA-256. At completion it rechecks device, inode, size, mtime, and
ctime on both the held descriptor and source path. Resume recomputes the full
SHA-256 and matches the durable source identity before trusting job state. An
unexpected change stops new work and is reported.

The program never changes source permissions, times, names, bytes, allocation,
or ownership. Corpus tests independently hash originals before and after every
run. The source must remain safe even if a parser worker crashes, the output
filesystem fills, or the supervisor is forcibly terminated.

Before starting, `split` estimates space for output parts, the current
temporary part, durable ledger and payload spool, independent-reader scratch,
manifests, and safety margin. A fresh job is refused below three times the
source size. Refinement may not weaken source or finalized-part safety.

## Recovery Model

The public report schemas use three independent state axes:

    RecoveryProvenance = normal | recovered | orphan | fragment
    ContentCompleteness = complete | partial | damaged
    ProcessingStatus = pending | spooled | written | unsupported | failed

`failed` is not a kind of recovered content, and `recovered` does not imply a
successful output. Each candidate has a stable `ItemKey` containing its
provenance, source node identifier when available, recovery index when
required, and an occurrence discriminator. Every item is accounted for once in
the final totals.

The canonical mail model preserves the item key, source folder path, message
class, subject, sender, recipients, sent/received times, text/HTML/RTF bodies,
internet headers, attachments, embedded messages, known MAPI values, unknown
typed properties that can be serialized safely, errors, and the three state
axes. Native pointers and source offsets are diagnostic data and never become
writer object identities.

Parsing occurs in supervised child processes because Rust cannot catch a
segmentation fault in `libpff`. A worker announces its current bounded unit
before processing it. The supervisor alone commits job state. After a crash it
restarts the worker, isolates the failing unit, records the failure after
bounded retries, and continues where the recovery API permits. Parser handles
are single-threaded; safe hashing, spool I/O, validation, and report work may
run concurrently with bounded queues.

## Splitting And PST Fidelity

Canonical items are ordered deterministically by normalized source folder
path, normal items before recovery-only items, source node identifier or
recovery index, and occurrence discriminator. The packer reproduces the
necessary folder hierarchy in every part and assigns each writable item to
exactly one part.

The visible folder path in an output part is the visible source path. PST
infrastructure nodes such as the store root and the IPM subtree are not emitted
as artificial parents, and PSTForge does not add a recovery wrapper around
ordinary reachable mail. Folder roles are identified from source metadata, not
display-name comparison: a user-created folder named `Deleted items` remains an
ordinary folder even when the store also has its well-known Deleted Items
folder. Source display names and hierarchy are otherwise preserved exactly.

The default part target remains 4 GiB. For every non-final normal part, the
packer takes the longest deterministic ordered prefix whose validated
serialized PST fits the requested size. It observes the difference between
estimated and actual PST size and extends or reduces that same prefix until the
next indivisible message would exceed the target. It does not publish
diagnostic halves or arbitrary large chunks. A normal non-final part may be
smaller than the target only by the serialized size of the next indivisible
message; the final remainder may be smaller. A message that cannot fit by
itself is preserved in a marked oversize part and makes the result partial.

Each output store receives a deterministic identity derived from the source
SHA-256, immutable job configuration, and part index. Node and block allocation
is deterministic. Source creation and modification timestamps are preserved
where valid; tool run time is stored in manifests, not substituted for source
mail time. Named property identifiers are rebuilt consistently per part when
the source GUID/name identity is available. The current `libpff` catalog
boundary exposes only the store-local `0x8000+` identifier, not that identity;
such source properties are omitted with explicit partial accounting rather
than guessed or assigned a semantically unrelated identity.

Known mail properties are translated to conforming PST structures. Unknown
properties are preserved when their MAPI type and value can be serialized
without ambiguity. An unsupported property is recorded on its item and does
not discard otherwise usable mail. An attachment failure leaves the parent
mail partial rather than failed when the remaining message can be written.
Data-less reference attachments preserve methods `2`, `3`, `4`, and `7`,
their nonempty long pathname or URL, an optional short pathname, and readable
web-provider permission metadata. PSTForge never opens, resolves, stats, or
fetches the referenced target. A missing or malformed required long pathname
omits only that attachment and makes the result partial. If damaged source
metadata asserts both a reference method and a readable by-value content
property, the reference relationship wins and the conflicting content
property is counted as an explicit omission, including when it is zero bytes.
OLE attachments preserve method `6`, the exact complete payload bytes, and
the source `PtypObject` or `PtypBinary` relationship. Readable attach-tag,
encoding, and static-rendition properties remain byte-exact when present;
PSTForge does not invent a rendition when it is absent. PSTForge never
instantiates, executes, repairs, converts, or dereferences an OLE object. A
complete zero-byte `PtypBinary` payload remains an exact readable source value.
A zero-byte `PtypObject` is malformed because its object descriptor cannot
reference a valid empty PST data block. A missing, incomplete, oversized, or
malformed required payload omits only that attachment and makes the result
partial. Malformed optional OLE metadata is omitted alone with explicit partial
accounting.
Readable binary attach-tag, encoding, and rendering metadata is also preserved
on complete by-value attachments when present; these properties are not
treated as method-6-only merely because they are commonly used by OLE objects.
PSTForge preserves a nonempty source attachment MIME type. When that property
is absent on a complete by-value attachment, PSTForge may derive a type from
an exact leading signature or a bounded structural parse under the confidence
rules in `docs/ATTACHMENT_RECOVERY.md`. ZIP is labeled as a generic container
unless its package metadata and required main part unambiguously prove DOCX,
XLSX, or PPTX. A Compound File Binary attachment is labeled DOC, XLS, or PPT
only when its readable directory has one unambiguous corresponding main
stream. It does not infer text types. For common Office files, a recognized
source filename extension can serve as corroborating recovery evidence only
when the payload is independently identified as the matching ZIP or CFB
container and no stronger structural evidence conflicts. Unrecognized data
remains unlabeled. When its source filename is also absent, the generated
recovery filename uses the extension of a payload-proven supported type, then
the extension of a recognized preserved source MIME type, and otherwise
`.bin`. Embedded Message objects use `.msg`. Generated filename evidence never
changes a nonempty source filename or replaces the preserved source MIME
property.

Writer inputs use typed recipient roles, body formats, attachment content,
named-property identities, and raw-property values. Named-property identifiers
are assigned store-wide after deterministic identity ordering, including
embedded messages and arbitrary GUID sets. Inline attachment position,
content ID, content location, and flags are explicit. RTF synchronization is
an input fact and is never inferred from the presence of RTF bytes. The native
body selector is also explicit and optional: it selects plain text, RTF, or
HTML only when that representation is present, and absence remains absence
rather than a derived preference. The version 0.2.1 typed writer boundary
accepts HTML bodies only as valid UTF-8 bytes and defaults
`PidTagInternetCodepage` to 65001. Version 0.4.0 preserves a positive source
Internet codepage and its byte-exact HTML; HTML declared as 65001, or lacking a
source codepage and therefore defaulted to 65001, must pass bounded streaming
UTF-8 validation or be omitted with partial accounting. The writer does not
synthesize the redundant `PidTagMessageCodepage` fallback. A raw property that
duplicates a writer-managed property is rejected at the writer boundary rather
than silently replacing the conforming value. Every intentionally omitted
property is returned in a structured write report with an empty top-level path
or the deterministic attachment-index path of its embedded message. The 0.2.1
canonical fixture boundary externalizes property values through 16 KiB and
rejects larger values; arbitrary payload streaming and packing is a 0.4.x
requirement.

Output creation is append/build-only in a `.partial` file. The writer flushes
all blocks, allocation maps, B-trees, tables, and headers, syncs the file,
closes it, validates it with internal and external readers, hashes it, writes
its sidecar manifest, syncs the directory, and atomically renames it to the
final `.pst` name. PSTForge never modifies a published part in place.

Every writer invariant is traceable through `docs/WRITER_CONFORMANCE.md` to an
authoritative Microsoft Open Specification or Microsoft MAPI requirement, the
implementing symbol, a focused test, and independent reader evidence.
Undocumented writer assumptions are release blockers. Reader acceptance and
ScanPST behavior supplement the normative source; they do not replace it.
Existing writer behavior must complete this audit before PSTForge admits
another source item class. An undocumented existing output is preserved while
it is audited; PSTForge does not strip completed behavior without an explicit
human decision based on its specification status and compatibility impact.

## Job Directory

The stable output structure is:

    job-dir/
      recovery.log
      parts/
        part-0001.pst
      .pstforge/
        job.sqlite3
        manifests/
          part-0001.json
        spool/
        partial/

`recovery.log` is the bounded human recovery record. It states what was
preserved, what was restored outside its original location, and what could not
be preserved, grouped by source-visible folder and plain-language reason. It
also reports bounded typed counts for source metadata that was derived from
other readable values, defaulted, or deliberately left absent. Across readable
message classes, a wholly missing subject or sender identity remains absent so
the importing client controls its presentation; associated items still require
a separate nonempty display name. If an associated item has neither a display
name nor subject, `(no subject)` is generated only for that structural display
name. These omissions and generated structural values are counted without
logging message values or item identifiers.
Part manifests contain size, SHA-256, store identity, counts, oversize status,
and bounded error totals under private job state rather than beside the PST
files. CLI `--json` output remains the machine-readable summary. The JSON
manifests, SQLite ledger, and spool are private implementation data and are not
an interchange format.

On successful completion, private spool payloads and stale partial output are
deleted unless `--keep-work` was specified. Empty private directories and the
ledger remain so reports and validation can be reproduced. Interrupted and
failed jobs retain enough work to resume.

Recovered property and attachment bytes are appended to one private,
job-local payload pack. The ledger stores checked pack offsets, lengths, and
SHA-256 values rather than payload copies or one file per property. The pack is
synced before a bounded candidate batch is committed; resume truncates any
uncommitted tail. SIGINT or SIGTERM checkpoints completed candidates in the
current batch, while abrupt process or host failure may replay only that
bounded batch. This private representation does not change output PST content.

Candidate and event metadata are consumed in one deterministic ordered pass.
PSTForge starts publishing validated parts from completed top-level messages
without first reconstructing the entire recovered mailbox in memory or through
per-candidate database queries.

Without `--keep-work`, packed payloads are securely removed and the ledger is
compacted after finalized parts and accounting are durable. Independent-reader
extraction scratch is created only beneath `.pstforge/partial/` on the selected
job filesystem and is removed after validation. PST construction checks for
interruption between streamed messages and blocks; an active independent
validator process group is terminated when interruption is requested. A
validator also arms parent-death containment before launching its reader, so
forced supervisor termination cannot leave the reader or its descendants
running against private job scratch.
Ledger integrity, migration, deletion, and compaction observe the same
interruption flag. An interrupted compaction remains durably marked pending and
is retried by the next matching resume.
Canonical catalog reconstruction, event and ownership reads, candidate
prefilter translation, and source-blob verification observe that flag as well.

## Reporting And Privacy

Split reports include source identity, recovery mode, invocation elapsed time,
logical source and finalized output bytes, average end-to-end source
throughput, and peak sampled RSS across the supervisor and parser workers.
Reports also include corruption observation, folders and candidates by
provenance, items by completeness and status, attachment totals, unsupported
item/property totals, exact aggregate rejection categories, part sizes and
hashes, retries, worker crashes, bounded error summaries, and whether the
source identity remained unchanged. A writer implementation limit is reported
as a product defect and does not establish that readable source data is
unrecoverable.

Default progress events report only operation state, part index, counts, byte
sizes, elapsed time, and interruption state. They never include mailbox names,
subjects, addresses, bodies, attachment names, or payload data.
Recovery emits a periodic active event while parser data is arriving. Parts
are published only after recovery traversal and candidate packing complete, so
an empty `parts/` directory during active recovery is not itself a stall.

Default logs use item keys, numeric node identifiers, operation names, and
error categories. They do not include subjects, addresses, bodies, attachment
names, attachment content, or raw properties. Full user data remains inside the
private spool and output PST files. JSON schemas include `schema_version` and
use decimal byte counts that safely represent values beyond 4 GiB.

The job-root `recovery.log` is mode `0600`, atomically replaced from durable
state, and never appended across resume. It uses human item descriptions and
source-visible folder paths, but excludes subjects, addresses, attachment
filenames, bodies, payloads, property identifiers, internal item keys, and
native parser jargon. Exact totals are never truncated. Folder-level detail is
limited to 10,000 lines and 4 MiB; additional groups are coalesced into exact
plain-language totals.

## Exit Status

    0   Every readable useful source value was preserved or safely regenerated
        in validated parts.
    1   Valid parts were produced, but some content was partial, unsupported,
        failed, or required an oversize part.
    2   Command-line usage was invalid.
    3   The source was invalid, changed, or could not be opened safely.
    4   Output, durable state, resume validation, or disk-space handling failed.
    5   A generated PST failed conformance validation.
    6   An internal invariant or unexpected implementation error occurred.
    130 The operator interrupted the run with SIGINT or SIGTERM after a durable
        checkpoint attempt.

No command returns success merely because it reached the end of traversal.

## 1.0 Acceptance

Release acceptance requires all local gates, the real external corpus, and the
50 GB corrupt PST. On the current x86_64 host, balanced recovery must finish
within 24 hours, keep peak PSTForge process RSS below 2 GiB, leave the source
unchanged, survive forced termination without losing finalized parts, and
account for every discovered mail item.

For the 19 GB qualification source, the readable inventory and the union of
unique item keys written across all finalized parts must match one-for-one.
Every readable top-level or embedded candidate must be written exactly once.
Any remaining unwritten item requires specific evidence that its source bytes
cannot be read or cannot be represented safely in a Unicode PST; a generic
writer rejection or unsupported status does not satisfy this exception.

The 19 GB operational qualification has a stricter 20-minute cold-run ceiling
on the current host, a six-minute maximum time to the first finalized part, and
the same 2 GiB aggregate RSS limit. A normal part is serialized once; the
implementation must not repeatedly rewrite a multi-gigabyte candidate merely
to discover whether it fits. Resume after material progress must complete less
work and finish faster than an equivalent cold restart. SIGINT and SIGTERM must
be observed at bounded data-processing intervals so a CPU-bound writer can
checkpoint and exit with status 130 promptly.

Every output part must pass PSTForge structural checks, Ubuntu/Debian
`pffinfo`, independent `readpst`, size and SHA-256 verification, and repeated
determinism tests. Representative healthy, partial, orphan, attachment, and
embedded-message cases must then import into a test Synology MailPlus mailbox
with matching folder and message counts and sampled content. Outlook opening
and inspection is a secondary release check.
