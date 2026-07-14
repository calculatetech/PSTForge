# Deliver a MailPlus-ready PST recovery and splitting utility

This ExecPlan is a living document. The sections `Progress`, `Surprises &
Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to
date as work proceeds.

Maintain this document in accordance with `.agent/PLANS.md`. A contributor must
be able to resume implementation using only the repository and this file. The
short roadmap is in `docs/ROADMAP.md`, the user-visible contract is in
`docs/PRODUCT_SPEC.md`, and agent workflow rules are in `AGENTS.md`. If those
documents disagree, stop implementation and reconcile them before committing.

## Purpose / Big Picture

PSTForge exists because a large damaged Outlook PST can be impossible to
import, repair, or split with existing Windows utilities. After 1.0, an
operator on Linux can point PSTForge at a damaged PST, keep the original
untouched, and receive a sequence of smaller valid PST files. Each part can be
imported independently into Synology MailPlus Server, so completed imports are
checkpoints and a problem in one part does not invalidate the rest.

The primary observable command is:

    pstforge split /data/damaged.pst \
      --output /recovery/damaged-job \
      --max-pst-size 4GiB

The command creates `parts/part-0001.pst` and subsequent parts, validates each
before making it visible, and writes human and JSON reports accounting for all
mail candidates. Stopping and rerunning with `--resume` continues compatible
work. Hash and identity evidence show that the source was not modified.

## Progress

- [x] (2026-07-14) Reviewed `docs/outline.md`, the repository state, host
  dependencies, `libpff` headers, licenses, Microsoft MS-PST material,
  Microsoft `outlook-pst` 1.2.0, and Synology MailPlus import documentation.
- [x] (2026-07-14) Locked the 1.0 product goal, CLI, version sequence,
  architecture, licensing boundaries, test strategy, and Git workflow in the
  documentation baseline.
- [ ] Milestone 0.1.0: Safe Foundation and Inspection.
- [ ] Milestone 0.1.1: Complete Mail Inventory.
- [ ] Milestone 0.2.0: Unicode PST Writer Foundation.
- [ ] Milestone 0.2.1: Mail-Fidelity PST Writer.
- [ ] Milestone 0.3.0: Recoverable Mail Pipeline.
- [ ] Milestone 0.3.1: Fault-Isolated Recovery.
- [ ] Milestone 0.4.0: Size-Limited PST Splitting.
- [ ] Milestone 0.4.1: Resume and 50 GB Qualification.
- [ ] Milestone 0.5.0: Operational UX and Debian Packaging.
- [ ] Milestone 0.5.1: GitHub CI and Private-Corpus Automation; the remote is
  reachable, and work begins after the approved baseline is pushed.
- [ ] Milestone 0.6.0: Interoperability Release Candidate.
- [ ] Milestone 1.0.0: MailPlus-Ready Release.

## Surprises & Discoveries

- Observation: `docs/outline.md` made PST splitting a non-goal, but the actual
  urgent requirement is to produce smaller PSTs for MailPlus import. EML,
  Maildir, PDF, contacts, and calendar exports are not 1.0 deliverables.
  Evidence: product decisions recorded on 2026-07-14 and
  `docs/PRODUCT_SPEC.md`.

- Observation: `libpff` exposes write flags but explicitly rejects write
  access. Adding PST output to it would be a new writer rather than filling in
  a small missing function.
  Evidence: upstream `libpff_file.c` returns "write access currently not
  supported" and its public API reserves `LIBPFF_ACCESS_FLAG_WRITE`.

- Observation: `libpff` is LGPL-3.0-or-later and permits modification, but it
  remains an alpha, single-parser-lane C dependency. Dynamic linking and
  process isolation minimize licensing and crash impact.
  Evidence: upstream `COPYING.LESSER`, Ubuntu package copyright, and upstream
  README, reviewed 2026-07-14.

- Observation: Microsoft publishes the MIT `outlook-pst` Rust crate, which
  already models and serializes low-level PST NDB and LTP structures but
  intentionally omits new-item creation and general modification.
  Evidence: `outlook-pst` 1.2.0 README and source at commit
  `1397836e73b690dbb09663f66056012fced45ff9`.

- Observation: The empty Unicode PST distributed with Microsoft's crate is
  accepted by both `pffinfo` 20231205 and independent `readpst` 0.6.76. This
  provides two external validators, but the runtime writer must create its own
  store and must not depend on a copied template.
  Evidence: local validation performed 2026-07-14.

- Observation: Debian 13 ships `libpff` 20180714 while Ubuntu 26.04 offers
  20231205. The older Debian header still contains every required corruption,
  recovery, orphan, recovered-item, record-set, and property API.
  Evidence: direct inspection of Debian package
  `libpff-dev_20180714-3.1+b2_amd64.deb` and Ubuntu package
  `libpff-dev_20231205-1build1_amd64.deb`.

## Decision Log

- Decision: PSTForge 1.0 writes smaller PSTs; general export formats move
  beyond 1.0.
  Rationale: MailPlus accepts PST directly and the user's immediate blocker is
  a 50 GB corrupt PST.
  Date/Author: 2026-07-14 / project owner and Codex.

- Decision: Output is new 64-bit Unicode PST version 23 with 512-byte pages and
  compressible permutation encoding.
  Rationale: This format supports large stores and has broad Outlook and
  third-party compatibility. New stores avoid all in-place repair risk.
  Date/Author: 2026-07-14 / Codex.

- Decision: Mail, folders, recipients, bodies, attachments, embedded messages,
  and supported raw properties are in scope. Other Outlook item classes are
  reported but not written before 1.0.
  Rationale: Synology MailPlus imports mail; broad MAPI item fidelity would
  delay the urgent recovery path.
  Date/Author: 2026-07-14 / project owner.

- Decision: Part size is user-configurable and defaults to 4 GiB. Date-range,
  folder-based, and other partition policies are post-1.0.
  Rationale: Size is the required import-checkpoint control. Four GiB yields a
  manageable number of parts for a 50 GB source.
  Date/Author: 2026-07-14 / project owner.

- Decision: Balanced recovery is the default; fragment scanning is explicit
  aggressive mode.
  Rationale: Balanced mode recovers normal, deleted, recovered, and orphan mail
  without the cost and false-positive risk of ignoring allocation metadata.
  Date/Author: 2026-07-14 / project owner.

- Decision: Use system `libpff` through a narrow dynamically linked FFI for
  input recovery, and adapt Microsoft `outlook-pst` 1.2.0 in a separately
  attributed MIT Rust crate for output.
  Rationale: `libpff` has mature damaged-item recovery but no writer. The
  Microsoft crate provides a safer, specification-linked writer foundation.
  Date/Author: 2026-07-14 / Codex.

- Decision: Recovery and writing are separated by a private transactional
  spool and SQLite job ledger.
  Rationale: A 50 GB job must resume after parser crashes or power loss, and a
  partially constructed PST can be rebuilt without reparsing completed source
  items.
  Date/Author: 2026-07-14 / Codex.

- Decision: PSTForge application code uses `Apache-2.0 OR MIT`; the adapted
  Microsoft writer crate remains MIT; `libpff` and any modifications remain
  LGPL-3.0-or-later.
  Rationale: This preserves upstream obligations and gives the Rust application
  a conventional permissive license.
  Date/Author: 2026-07-14 / project owner.

- Decision: GitHub-dependent work starts only after the approved documentation
  baseline is pushed; it is not a prerequisite for local implementation and
  testing.
  Rationale: The remote and `gh` authentication became available on
  2026-07-14. GitHub work follows the approved baseline, while local quality
  gates remain independently useful.
  Date/Author: 2026-07-14 / project owner and Codex.

## Outcomes & Retrospective

The documentation baseline replaces an export-oriented outline with a
decision-complete route to MailPlus-ready PST parts. Implementation has not
started. Update this section at every major milestone with what a user can now
do, evidence that acceptance passed, remaining gaps, and lessons that change
later work.

## Context and Orientation

The repository initially contains only documentation. `AGENTS.md` governs all
agent work. `docs/PRODUCT_SPEC.md` is the stable 1.0 behavioral contract.
`docs/ROADMAP.md` is the short version sequence. This file supplies the
implementation detail. `docs/outline.md` is a superseded source brief and must
not override newer decisions.

A PST is a Microsoft Personal Storage Table file. Its Node Database (NDB)
stores blocks and B-trees, its Lists and Tables layer (LTP) stores property and
table contexts, and its Messaging layer represents stores, folders, messages,
recipients, and attachments. Splitting is not byte slicing: each output part
must rebuild all three layers into a self-contained new PST.

`libpff` is a C library for reading healthy and damaged PSTs. It can identify
corruption, traverse folders and items, recover deleted items, enumerate
orphans, and scan fragments. It cannot write PSTs and does not promise
multithreading. Keep its handles inside one worker process and one parser lane.

The durable spool is private recovery state under the job directory. It stores
canonical metadata and streamed body/attachment blobs so source reading and
PST construction can resume independently. The ledger is a bundled SQLite
database owned only by the supervisor. A finalized part is immutable.

The planned workspace is:

    Cargo.toml
    crates/
      pstforge-cli/       clap commands and presentation
      pstforge-core/      domain models, recovery orchestration, packing
      libpff-sys/         pkg-config linking, bindings, all unsafe code
      pstforge-pst/       MIT PST writer adapted from outlook-pst 1.2.0
      pstforge-job/       SQLite ledger and spool
      xtask/              local and CI automation
    tests/
      corpus-manifest.example.toml
      corpus-schema.json

Keep the binary name `pstforge`. The CLI crate depends on safe interfaces from
the other crates and never calls FFI directly. The core crate depends on traits
so unit tests can use fake recovery and writer implementations.

## Product Interfaces

The CLI and output contracts are defined fully in `docs/PRODUCT_SPEC.md`. The
implementation must expose these clap commands without aliases that create a
second behavior path:

    pstforge info <source.pst> [--json]
    pstforge verify <source.pst> [--mode full|recovery] [--json]
    pstforge split <source.pst> --output <job-dir> \
      [--max-pst-size <size>] [--recovery balanced|aggressive] \
      [--resume] [--keep-work] [--json]
    pstforge report <job-dir> [--json]

Every durable JSON object has `schema_version`. Use unsigned 64-bit integers in
Rust for byte counts and counters and decimal JSON numbers only where values
remain exactly representable by the documented consumers. Use strings for
hashes, typed enums serialized in lower snake case, RFC 3339 UTC timestamps,
and stable field names.

In `pstforge-core`, define domain types equivalent to:

    pub enum RecoveryProvenance { Normal, Recovered, Orphan, Fragment }
    pub enum ContentCompleteness { Complete, Partial, Damaged }
    pub enum ProcessingStatus {
        Pending, Spooled, Written, Unsupported, Failed,
    }

    pub struct ItemKey {
        pub provenance: RecoveryProvenance,
        pub source_node_id: Option<u32>,
        pub recovery_index: Option<u64>,
        pub occurrence: u32,
    }

    pub struct CanonicalMail {
        pub key: ItemKey,
        pub folder_path: Vec<String>,
        pub message_class: Option<String>,
        pub subject: Option<String>,
        pub sender: Option<Mailbox>,
        pub recipients: Vec<Recipient>,
        pub sent_at: Option<DateTime<Utc>>,
        pub received_at: Option<DateTime<Utc>>,
        pub bodies: MessageBodies,
        pub internet_headers: Vec<HeaderField>,
        pub attachments: Vec<CanonicalAttachment>,
        pub raw_properties: Vec<RawMapiProperty>,
        pub provenance: RecoveryProvenance,
        pub completeness: ContentCompleteness,
        pub errors: Vec<ItemError>,
    }

Store body and attachment payloads as content-addressed spool references, not
unbounded `Vec<u8>` fields. Unknown MAPI properties retain numeric property ID,
property type, optional named-property identity, raw length, and a typed or blob
value. Do not serialize a value when its declared length or type is invalid.

The safe input boundary implements:

    pub trait RecoverySource {
        fn inspect(&mut self) -> Result<SourceInspection, RecoveryError>;
        fn inventory(&mut self, sink: &mut dyn CandidateSink)
            -> Result<InventorySummary, RecoveryError>;
        fn recover(&mut self, mode: RecoveryMode,
                   sink: &mut dyn CandidateSink)
            -> Result<RecoverySummary, RecoveryError>;
    }

`CandidateSink` must accept one bounded candidate at a time and return only
after its durable spool transaction commits. The worker protocol carries
versioned length-delimited messages over inherited pipes; never use subjects or
paths as protocol identifiers. The supervisor records `started`, `committed`,
and failure events and enforces bounded retries.

The writer boundary implements:

    pub trait PstPartWriter {
        fn create(path: &Path, identity: StoreIdentity,
                  limits: WriterLimits) -> Result<Self, WriteError>
        where Self: Sized;
        fn ensure_folder(&mut self, path: &[String])
            -> Result<FolderId, WriteError>;
        fn write_mail(&mut self, folder: FolderId,
                      mail: &CanonicalMail,
                      blobs: &dyn BlobSource)
            -> Result<WrittenMail, WriteError>;
        fn finish(self) -> Result<UnvalidatedPart, WriteError>;
    }

`finish` writes headers, maps, block and node B-trees, folder hierarchy and
contents tables, name-to-ID mapping, and all CRC/signature fields, then syncs
and closes the `.partial` file. It cannot return a published part. A separate
validator returns `ValidatedPart`, and only that type can be atomically renamed
and committed to the ledger.

## Plan of Work

### Milestone 1: Version 0.1.0 - Safe Foundation and Inspection

Create the workspace and license files. Pin an MSRV supported by Debian 13 and
the chosen dependency versions; initially use Rust 1.85 unless compilation
research proves a lower or higher floor is necessary, then record the evidence
here before changing it. Implement `libpff-sys` with allowlisted, checked-in
bindings compatible with `libpff` 20180714 and 20231205. `build.rs` uses
`pkg-config` for dynamic linking and emits an actionable missing-package error.
No normal build requires bindgen or libclang.

Wrap native errors and ownership in safe RAII types. Check every native return,
pointer, length, and conversion. Open sources only after rejecting symlinks and
unsafe output relationships. Implement source metadata and streaming SHA-256,
then `info` and quick/full inspection foundations. Create `xtask` and the fake
backend before adding real corpus assertions.

Acceptance: `cargo xtask gate fast` passes; `pstforge info` reports a healthy
external PST in human and JSON forms; `verify --mode full` inventories a small
healthy PST; pre/post SHA-256 and identity metadata match exactly.

### Milestone 2: Version 0.1.1 - Complete Mail Inventory

Implement bounded folder traversal and known/raw property extraction. Copy
native strings and values into owned Rust types before freeing items. Stream
large bodies and attachments into a temporary candidate sink. Detect cycles,
duplicate node references, invalid sizes, depth exhaustion, and unsupported
message classes. Preserve embedded-message relationships without recursive
stack growth.

Acceptance: external healthy ANSI and Unicode PSTs match manifest invariants
for folders, messages, recipients, and attachments; peak memory remains bounded
during a large-attachment case; no corpus source changes.

### Milestone 3: Version 0.2.0 - Unicode PST Writer Foundation

Import the required Microsoft `outlook-pst` 1.2.0 code into
`crates/pstforge-pst`, retaining MIT attribution and documenting the pinned
commit in `UPSTREAM.md`. Extend it to create a new store without a template:
write the version 23 header, root, allocation maps, initial NBT and BBT, message
store, name-to-ID map, IPM subtree, root folder, Deleted Items, hierarchy and
contents tables. Add allocation, B-tree insertion/splitting, heap, property
context, table context, CRC, and block-signature tests directly tied to MS-PST
sections.

Write one folder and one plain-text message. Validate with the writer's
structural checker, `pffinfo`, and `readpst`. A MailPlus smoke import of this
small generated PST is the promotion gate; if it fails, keep the milestone
active, record the exact rejection, and fix the writer rather than proceeding
with an unvalidated format.

### Milestone 4: Version 0.2.1 - Mail-Fidelity PST Writer

Implement recipients, Unicode subject/address values, Internet headers, text,
HTML and compressed RTF bodies, attachments, embedded messages, folder
contents tables, associated counts, and named properties. Map required MAPI
properties explicitly. Preserve safely serializable unknown properties and
record unsupported ones. Generate deterministic store identifiers and node
allocation from immutable job inputs.

Acceptance: round-trip canonical comparisons pass through both `libpff` and
independent `readpst`; attachment hashes and sampled source properties match;
repeated writes are byte-identical; MailPlus displays folder, sender,
recipient, subject, body, timestamp, and attachment samples correctly.

### Milestone 5: Version 0.3.0 - Recoverable Mail Pipeline

Implement the SQLite ledger and content-addressed spool. Use WAL during active
work, full synchronous transactions for item commits, integrity checks at open,
and a final checkpoint before reporting a stable state. Invoke normal
traversal, default `libpff_file_recover_items`, recovered-item enumeration, and
orphan enumeration. Deduplicate only when stable source identity proves two
enumerations reference the same source object; do not content-deduplicate
distinct messages.

Acceptance: deleted, recovered, and orphan corpus cases produce correct
provenance and completeness totals; killing the process after a committed item
does not lose that item; one corrupt attachment leaves a writable partial
message when possible.

### Milestone 6: Version 0.3.1 - Fault-Isolated Recovery

Move all native parsing to a hidden worker subcommand with a versioned IPC
protocol. The supervisor records which bounded unit is starting, monitors
exit/signal status, restarts after crashes, narrows a failed batch until it can
identify the smallest addressable item, and continues. Limit retries to three
per identical unit and record the final failure. For recovered-index work that
requires repeating a `libpff` scan, reuse completed spool entries and explain
the rescan cost in progress output.

Add aggressive mode with `IGNORE_ALLOCATION_DATA` and
`SCAN_FOR_FRAGMENTS`. Keep fragment results distinct and lower-confidence.
Acceptance includes injected worker aborts, segmentation faults in a test
shim, malformed lengths, stalled workers, graceful SIGINT/SIGTERM, and a
source parser error after earlier items committed.

### Milestone 7: Version 0.4.0 - Size-Limited PST Splitting

Implement deterministic candidate ordering and a packer that estimates full
PST overhead, retains a safety reserve, writes a temporary part, and validates
actual size. If a normal part exceeds the target, repartition and rebuild
before publication. If one item alone exceeds the target, write an oversize
part and return partial success. Reproduce only folders required by a part,
plus mandatory store folders, and keep every mail item in exactly one part.

Finish, sync, close, structurally validate, run configured external validators,
hash, write the sidecar, sync, and atomically rename each part. Commit the part
and item assignments in one ledger transaction after publication. Never edit a
published part.

Acceptance: boundary sizes, table-growth boundaries, one-byte-over cases,
folder replication, oversize mail, deterministic assignments, and forced
termination at every publication step leave only valid finalized parts or
discardable `.partial` files.

### Milestone 8: Version 0.4.1 - Resume and 50 GB Qualification

Implement immutable job configuration and source matching, automatic state
integrity checks, `--resume`, `--keep-work`, stale partial cleanup, progress,
throughput/resource metrics, and conservative disk-space preflight. A resume
may continue only if source SHA-256 and identity, recovery mode, maximum size,
writer format, schema, and compatible tool major version match.

Run the 50 GB corrupt PST in balanced mode on the current host. Capture bounded
evidence under `.agent/test-results/`, interrupt it normally and with SIGKILL,
resume it, validate every part, and verify source identity. Acceptance is
completion within 24 hours, peak process RSS below 2 GiB, no loss of finalized
parts, and final accounting for every discovered mail candidate.

### Milestone 9: Version 0.5.0 - Operational UX and Debian Packaging

Finalize report schemas, privacy redaction, exit statuses, error wording,
report regeneration, install diagnostics, spool cleanup, and operator docs.
Create a reproducible Debian 13 x86_64 `.deb` that dynamically depends on
`libpff1t64` at the Debian-compatible floor. Run build and integration tests in
a clean Debian 13 environment and on Ubuntu 26.04.

Acceptance: installing the package on clean Debian 13 makes `pstforge info`,
`verify`, `split`, and `report` work with only declared dependencies; removing
it leaves user jobs and source PSTs untouched; package contents, licenses, and
dependency metadata pass inspection.

### Milestone 10: Version 0.5.1 - GitHub CI and Private-Corpus Automation

Begin after the human-approved documentation baseline is pushed to the
reachable `origin` remote and repository settings can be configured. This
repository does not use pull requests. Add branch-push workflows for formatting,
clippy, unit/integration tests, Debian and Ubuntu builds, docs, license policy,
and advisories. Add scheduled fuzzing and a manual self-hosted runner labeled
for the private PST corpus. The private runner emits only redacted
JUnit/summary data and never uploads PSTs, spool data, mail metadata, or verbose
logs.

Add a release workflow that builds but cannot publish without an approved tag
and environment. Repository automation does not waive the rule that agents may
not push, merge, tag, or publish without explicit human approval.

### Milestone 11: Version 0.6.0 - Interoperability Release Candidate

Freeze CLI/schema changes, run every local and GitHub gate, fuzz parsers and
writer structures, inject disk and process failures, and complete security,
license, privacy, and data-loss reviews. Import representative parts into a
clean MailPlus test mailbox and compare folder/message counts and sampled
content. Open the same parts in supported Outlook as a secondary test.

No blocker or high-severity review finding may remain. Medium findings must be
fixed or explicitly accepted by the human owner in the Decision Log. A release
candidate is not 1.0 until the real 50 GB rehearsal succeeds from clean state.

### Milestone 12: Version 1.0.0 - MailPlus-Ready Release

From a clean Debian package install, repeat balanced recovery of the real 50 GB
source, validate all parts, and import them into MailPlus. Confirm source
identity, complete accounting, resume behavior, performance limits, package
reproducibility, documentation, and licensing. Record bounded conclusions and
artifact hashes. After adversarial review is clean, create the local release
commit. Do not merge, tag, push, or publish until explicit human approval.

## Concrete Steps

All commands run from the active milestone worktree, never from `main`. Before
implementation, the human-approved documentation commit must be established as
`main`. For milestone 0.1.0, create the worktree with:

    mkdir -p ../pstforge-worktrees
    git worktree add \
      -b milestone/v0.1.0-safe-foundation \
      ../pstforge-worktrees/v0.1.0-safe-foundation main
    cd ../pstforge-worktrees/v0.1.0-safe-foundation

On Ubuntu 26.04, install the currently missing development and independent
validation packages with:

    sudo apt update
    sudo apt install --yes libpff-dev pff-tools pst-utils

The host already has `build-essential`, `pkg-config`, `git`, `rustc`, `cargo`,
and `clang`. Required Ubuntu candidates verified on 2026-07-14 are
`libpff-dev` and `pff-tools` 20231205-1build1 and `pst-utils` 0.6.76-1.3.
`libpff-dev` is the build requirement; `pff-tools` and `pst-utils` are
development validators. Do not require `libclang-dev` in normal builds because
bindings are checked in. Recheck candidate names and versions if the host
release changes.

The external corpus manifest is never committed. Create it outside the
repository, for example at
`$XDG_DATA_HOME/pstforge-test-corpus/manifest.toml`, and export:

    export PSTFORGE_CORPUS_MANIFEST="$XDG_DATA_HOME/pstforge-test-corpus/manifest.toml"
    export PSTFORGE_TEST_RESULTS="$PWD/.agent/test-results"

The example manifest in `tests/` defines required fields without real paths or
mail metadata. Each real entry records an opaque case ID, absolute path,
SHA-256, source format, classification, expected invariant ranges, and allowed
test tiers. Include healthy ANSI, healthy Unicode, real-world corruption,
orphan/deleted content, large attachments, embedded messages, malformed
HTML/RTF, and the private 50 GB case. Derived corruption files belong in an
external scratch directory and must be created from a hash-verified copy or
reflink.

As `xtask` becomes available, use:

    cargo xtask gate fast
    cargo xtask gate full
    cargo xtask gate release

`fast` runs `cargo fmt --check`, workspace `cargo check`, clippy with warnings
denied, unit tests, schema tests, and documentation-link checks. `full` adds
integration tests, external small/medium corpus cases, `pffinfo`, `readpst`,
fault injection, source immutability, deterministic-output, license policy,
and advisory checks. `release` adds clean Debian packaging, the large corpus,
MailPlus/Outlook evidence checks, reproducibility, and release documentation.
Tool output is summarized to the terminal and written in detail only under the
untracked test-results directory.

At each milestone stopping point:

    git status --short
    git diff --check
    git diff --stat
    cargo xtask gate full

Review the entire diff adversarially. Resolve all blocker/high findings and
rerun the affected gates. Update this file's living sections. Only then create
a focused local commit naming the version. Do not push or merge until the human
owner explicitly approves those separate actions.

## Validation and Acceptance

Unit tests cover FFI return mapping with a shim, RAII cleanup, integer and
length bounds, domain transitions, packer boundaries, deterministic IDs,
writer byte structures, CRC/signature calculations, ledger transactions,
resume matching, CLI parsing, JSON schemas, and privacy redaction.

Integration tests cover healthy ANSI and Unicode PSTs, normal hierarchy,
deleted/recovered/orphan mail, large attachments, embedded messages, malformed
bodies, unsupported MAPI types, truncated files, damaged B-trees and allocation
maps, source symlinks, source-under-output, permissions, disk exhaustion,
worker crashes/stalls, SIGINT/SIGTERM/SIGKILL, ledger corruption, incompatible
resume, size boundaries, an oversize item, and deterministic reruns.

Every source-bearing test records hash and identity before and after. A mismatch
is a test failure even if output is otherwise valid. Every generated part must
pass internal structural validation, `pffinfo`, and `readpst`; tests compare
folder/message counts, source item keys, important MAPI values, and attachment
hashes through independent reads. Do not accept writer self-round-trip alone.

The human MailPlus acceptance procedure uses a dedicated test user, imports
each representative PST as a new mailbox, selects a documented duplicate
policy, and records part hash, imported folder/message counts, errors, and
sampled body/attachment results without copying private content into the
repository. Outlook checks open the part, expand folders, sample content and
attachments, and run any locally available integrity check.

Release acceptance is exactly the behavior in `docs/PRODUCT_SPEC.md`: all
selected mail is accounted for; partial/unsupported/failed content is explicit;
valid parts are independently importable; resume is durable; the 50 GB balanced
run finishes within 24 hours and below 2 GiB process RSS on the current host;
and the source remains unchanged.

## Idempotence and Recovery

Inspection and verification are read-only and repeatable. A fresh `split`
refuses an existing nonempty job directory. `split --resume` only operates on a
fully matching immutable configuration and source. It never guesses that two
jobs are compatible.

Spool blobs are written to temporary names, synced, atomically renamed, then
referenced by a committed ledger transaction. Orphan temporary blobs can be
removed after an integrity scan. Published PSTs are never reopened for write.
An unvalidated `.partial` file may always be deleted and rebuilt from committed
spool items. If the ledger fails integrity checks, stop and preserve evidence;
do not reconstruct state from filenames automatically.

After a worker crash, the supervisor uses the last announced unit and committed
ledger state to retry. After three identical failures it records the smallest
addressable failing unit and continues when possible. If global `libpff`
recovery itself always crashes before candidates can be addressed, normal-tree
output remains valid and the job ends partial with the blocker recorded.

If the source identity changes, stop assigning work and refuse resume. If disk
space runs out, stop workers, keep committed spool and finalized parts, remove
only known temporary files, and return output failure. If external validation
rejects a part, retain the `.partial` file and bounded diagnostics, do not
publish it, and return validation failure.

## Artifacts and Notes

Keep detailed test output, timings, crash traces, corpus paths, and MailPlus
screenshots outside version control under `.agent/test-results/` or another
private evidence location. The ExecPlan records only short conclusions and
hashes needed to understand decisions. Never record message subjects, sender
addresses, bodies, attachment names/content, or private absolute paths in a
committed artifact.

The output JSON schemas, external corpus example/schema, upstream attribution,
license texts, Debian metadata, and operator instructions are durable tracked
artifacts. Generated PSTs, actual corpus manifests, SQLite jobs, spool blobs,
package build output, fuzz corpora derived from private mail, and test logs are
not tracked.

## Interfaces and Dependencies

Use Rust, Cargo, `clap`, `tracing`, `serde`, `serde_json`, `thiserror`, `anyhow`
at executable boundaries, `sha2`, `chrono`, and `rusqlite` with bundled SQLite.
Select exact versions during milestone 0.1.0, commit `Cargo.lock`, deny unknown
registries and git sources except a documented temporary upstream research
pin, and record every license in policy. Avoid dependencies for byte parsing or
size strings when a small audited implementation or mature existing crate
already in the graph suffices.

Use dynamically linked `libpff` with a minimum supported API corresponding to
20180714. At startup report the detected version. Reject a library missing a
required symbol with an installation diagnostic. Ubuntu development uses
20231205. Debian 13 compatibility is a required build/test lane.

The PST writer implements Microsoft MS-PST revision 11.2. The adapted
`pstforge-pst` crate remains MIT and includes its upstream commit and changes.
PSTForge application crates are `Apache-2.0 OR MIT`. Include LGPL and libpff
notices in binary/package documentation. If a proven corpus failure requires a
`libpff` change, make it in a separate LGPL fork and branch after the approved
baseline is pushed, prefer an upstreamable patch, publish corresponding source
with any distributed binary, and preserve runtime library replacement.

GitHub Actions, remote forks, release environments, badges, branch protection,
and self-hosted runner configuration build on the approved remote baseline.
Recheck authentication and repository settings before creating or configuring
them. Local work must never depend on their existence.

Revision note (2026-07-14): Initial decision-complete ExecPlan created from the
reviewed outline, host/package inspection, upstream source and license review,
and owner decisions prioritizing mail-only PST output for Synology MailPlus.
