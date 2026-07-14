# PSTForge 1.0 Product Specification

## Purpose

PSTForge is a Linux command-line recovery utility for operators who have a
large or damaged Outlook Personal Storage Table (PST) that existing tools
cannot process reliably. It reads the source without modification, accounts
for reachable and recoverable mail, and produces smaller PST files that can be
imported independently into Synology MailPlus Server. Smaller parts provide
restart and import checkpoints and limit the scope of a failed import.

Synology MailPlus is the primary 1.0 consumer. Outlook is a secondary
compatibility check. PSTForge is noninteractive and suitable for unattended
runs, while retaining concise human progress and machine-readable reports.

## Scope

PSTForge 1.0 supports healthy and damaged ANSI and Unicode PST input that
`libpff` can open. It recovers the normal folder tree, email, recipients,
message bodies, attachments, embedded messages, deleted or disconnected mail
exposed by `libpff`, orphan items, and fragment candidates in explicit
aggressive mode.

PSTForge 1.0 always writes new 64-bit Unicode PST version 23 files with
512-byte pages. Output uses the format's compressible permutation encoding for
broad compatibility; this encoding is obfuscation, not password protection.
Each part is a complete PST with its own message store identity and required
folder tables.

The following are outside 1.0: editing or repairing the source, PST merging,
OST conversion, password cracking, contacts, calendars, tasks, notes, journal
items, distribution lists, EML, Maildir, PDF, general attachment export, GUI
work, date-range partitioning, and folder-based partitioning. Unsupported
source items are counted and reported.

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

### `pstforge split <source.pst> --output <job-dir> [OPTIONS]`

`split` is the primary command. Its stable 1.0 options are:

    --max-pst-size <SIZE>        Default: 4GiB. Accept IEC or SI suffixes.
    --recovery <MODE>            balanced (default) or aggressive.
    --resume                     Resume a matching interrupted job.
    --keep-work                  Retain the private spool after success.
    --json                       Print the final summary as JSON.

Without `--resume`, an existing nonempty job directory is an error. With
`--resume`, the source SHA-256, source identity, job schema, recovery mode,
part-size policy, writer format, and compatible tool major version must match.
A mismatch is refused and neither existing parts nor state are changed.

Balanced recovery processes the normal tree, `libpff` deleted/recovered items,
and orphan items. Aggressive recovery also sets the `libpff` flags to ignore
allocation data and scan fragments. Aggressive candidates retain lower
confidence and are never presented as ordinary reachable mail.

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
time, and SHA-256. At completion and resume it rechecks identity metadata; an
unexpected change stops new work and is reported.

The program never changes source permissions, times, names, bytes, allocation,
or ownership. Corpus tests independently hash originals before and after every
run. The source must remain safe even if a parser worker crashes, the output
filesystem fills, or the supervisor is forcibly terminated.

Before starting, `split` estimates space for the durable spool, output parts,
the current temporary part, manifests, and safety margin. By default it refuses
when available space is below three times the source size. This conservative
preflight is reported explicitly; a later implementation may refine the
estimate without weakening source or finalized-part safety.

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

Each output store receives a deterministic identity derived from the source
SHA-256, immutable job configuration, and part index. Node and block allocation
is deterministic. Source creation and modification timestamps are preserved
where valid; tool run time is stored in manifests, not substituted for source
mail time. Named property identifiers are rebuilt consistently per part.

Known mail properties are translated to conforming PST structures. Unknown
properties are preserved when their MAPI type and value can be serialized
without ambiguity. An unsupported property is recorded on its item and does
not discard otherwise usable mail. An attachment failure leaves the parent
mail partial rather than failed when the remaining message can be written.

Output creation is append/build-only in a `.partial` file. The writer flushes
all blocks, allocation maps, B-trees, tables, and headers, syncs the file,
closes it, validates it with internal and external readers, hashes it, writes
its sidecar manifest, syncs the directory, and atomically renames it to the
final `.pst` name. PSTForge never modifies a published part in place.

## Job Directory

The stable output structure is:

    job-dir/
      manifest.json
      report.json
      report.txt
      parts/
        part-0001.pst
        part-0001.json
      .pstforge/
        job.sqlite3
        spool/
        partial/

`manifest.json` records schema version, immutable options, source identity,
tool and library versions, job state, part summaries, and aggregate counts.
`report.json` is the machine recovery report. `report.txt` is the concise human
summary. Part sidecars contain size, SHA-256, store identity, counts,
oversize status, and bounded error totals. The SQLite ledger and spool are
private implementation data and are not an interchange format.

On successful completion, the spool and partial directory are deleted unless
`--keep-work` was specified. The ledger remains so reports and validation can
be reproduced. Interrupted and failed jobs retain enough work to resume.

## Reporting And Privacy

Reports include source identity and format, corruption observation, recovery
mode, elapsed time, peak memory, bytes read/written, folders and candidates by
provenance, items by completeness and status, attachment totals, unsupported
item/property totals, part sizes and hashes, retries, worker crashes, bounded
error summaries, and whether the source identity remained unchanged.

Default logs use item keys, numeric node identifiers, operation names, and
error categories. They do not include subjects, addresses, bodies, attachment
names, attachment content, or raw properties. Full user data remains inside the
private spool and output PST files. JSON schemas include `schema_version` and
use decimal byte counts that safely represent values beyond 4 GiB.

## Exit Status

    0   All selected mail was written to validated parts.
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

Every output part must pass PSTForge structural checks, Ubuntu/Debian
`pffinfo`, independent `readpst`, size and SHA-256 verification, and repeated
determinism tests. Representative healthy, partial, orphan, attachment, and
embedded-message cases must then import into a test Synology MailPlus mailbox
with matching folder and message counts and sampled content. Outlook opening
and inspection is a secondary release check.
