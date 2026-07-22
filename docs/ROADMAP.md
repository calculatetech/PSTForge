# PSTForge Roadmap to 1.0

## Goal

PSTForge 1.0 will run on Linux, read a damaged Microsoft Outlook PST without
modifying it, recover as much mail as can be handled safely, and write smaller
independently valid PST files for import into Synology MailPlus Server. Clean
ScanPST analysis followed by successful Microsoft Outlook inspection is the
required interoperability acceptance boundary; MailPlus compatibility is then
assumed.

Each milestone has one observable result and one version. A new feature
category increments the second version digit. A related feature increments the
third digit. Fixes made before a milestone is committed remain at that
milestone's version. The final compatibility and safety gate produces 1.0.0.

## Milestones

### 1. Version 0.1.0 - Safe Foundation and Inspection

Create the Rust workspace, read-only `libpff` integration, source identity and
SHA-256 handling, structured diagnostics, local automation, and the `info` and
basic `verify` commands. Demonstrate that a healthy external PST can be opened,
identified, inspected, and closed without changing its hash or metadata.

### 2. Version 0.1.1 - Complete Mail Inventory

Stream the normal folder hierarchy, email messages, recipients, bodies,
attachments, embedded messages, and supported raw MAPI properties into a
canonical catalog. Demonstrate complete accounting on healthy ANSI and Unicode
PSTs without loading the entire file or an unbounded attachment into memory.

### 3. Version 0.2.0 - Unicode PST Writer Foundation

Adapt Microsoft `outlook-pst` 1.2.0 to create a new 64-bit Unicode version 23
PST programmatically. Write a minimal store, folder, and message, then prove it
is structurally valid with internal checks, `pffinfo`, and independent
`readpst`. No copied template file may be required at runtime.

### 4. Version 0.2.1 - Mail-Fidelity PST Writer

Write folder tables, messages, recipients, plain and HTML bodies, attachments,
embedded messages, named properties, and supported raw properties. Preserve
source mail semantics, generate new store identities, and make repeated writes
from identical canonical input byte-for-byte deterministic where the format
permits.

### 5. Version 0.3.0 - Recoverable Mail Pipeline

Add normal, deleted, recovered, and orphan item extraction through `libpff`.
Record how every candidate was found, how complete its content is, and what was
written. Demonstrate that one damaged message or attachment does not stop
unrelated mail from reaching a private transactional ledger and
content-addressed recovery spool. Expose balanced recovery as a durable job;
PST part creation remains 0.4.0.

### 6. Version 0.3.1 - Fault-Isolated Recovery

Run native parsing in supervised workers, checkpoint after bounded units of
work, contain crashes, and retry or isolate failing items without losing prior
progress. Add aggressive recovery that scans fragments and disregards suspect
allocation metadata while keeping balanced recovery as the default.

### 7. Version 0.4.0 - Size-Limited PST Splitting

Implement deterministic packing into independently valid PST parts with a
user-configurable maximum and a 4 GiB default. Replicate the required folder
hierarchy in each part without artificial recovery or store-root wrappers,
assign every writable message exactly once, and fill each normal non-final part
to the target unless the next indivisible message would exceed it. Validate
before atomic publication, and preserve an indivisible oversize message in a
clearly marked oversize part.

### 8. Version 0.4.1 - Resume and 50 GB Qualification

Extend the transactional job ledger and private recovery spool with explicit
compatible resume, graceful interruption, progress, disk-space checks, and
cleanup. Qualify first with the owner's 19 GB corrupt PST to reduce test-data
movement, then run the real 50 GB corrupt PST end to end. Show that forced
termination cannot damage the source or already finalized parts. The 19 GB
qualification must finish within the owner's 20-minute operational target on
the current host. Version 0.4.1 records measured peak RSS; the owner accepted
the 5,317,328,896-byte result as a known limitation. The former 2 GiB objective
remains historical optimization evidence and is not a 1.0 release blocker.
Content fidelity findings discovered during qualification are planned
separately as version 0.4.2 and are not folded into the accepted splitting
branch.

### 9. Version 0.4.2 - Incremental Data Correctness

Remove policy-based content loss. Preserve recursive embedded attachments and
every readable native PST item class, useful property, named-property identity,
folder/store metadata, and associated item that another PST reader could use.
Keep `parts/` PST-only and write one bounded human `recovery.log` explaining
restored and unpreserved data. Deliver the milestone through reviewed,
human-validated checkpoint commits on one 0.4.2 branch, using one focused PST
per checkpoint before a single final 19 GB run. Before admitting another item
class, audit the complete existing writer against authoritative Microsoft
specifications in `docs/WRITER_CONFORMANCE.md`; every remaining writer change
must add its normative reference, implementation symbol, focused test, and
independent evidence before implementation.

The focused correctness checkpoints passed. The final cold 19 GB reconciliation
was stopped after more than 57 minutes with only one finalized part because the
expanded fidelity path repeatedly serialized multi-gigabyte trial parts and
exceeded 6 GiB RSS. That is recorded as a failed performance gate, not a
successful scale result, and is the immediate scope of version 0.4.3.

### 10. Version 0.4.3 - Incremental Writer Performance

Replace whole-mailbox materialization and repeated whole-part trial writes with
bounded transactional PST construction. Append complete messages
incrementally, use actual allocation plus bounded finalization headroom to
decide whether the next indivisible message fits, and serialize each normal
part once. Preserve independent validation, atomic publication, source safety,
bounded memory, durable restart state, and materially faster resume than a
cold restart. The accepted restartable implementation retains a private
payload spool and therefore costs approximately one extra full
readable-payload write and temporary allocation.

Restore the 19 GB cold-run ceiling to one minute per source GiB on the current
host, target the previously demonstrated approximately ten-minute result, and
keep aggregate PSTForge RSS below 2 GiB. Complete the deferred 0.4.2 whole-job
reconciliation across independently validated 4 GiB splits before proceeding.
A low-write direct-output mode remains future work after the newly exposed
native-item data-correctness gaps are resolved.

### 11. Version 0.4.4 - Whole-Job Data Reconciliation

Complete the deferred 19 GB input/output reconciliation. Every readable native
candidate, including embedded items, must appear exactly once across the
generated PST parts. A writer limitation is a defect, not evidence that source
data is unrecoverable. Remove the known empty-body, recipient-table heap,
general heap, raw-property, and aggregate-recipient rejection paths.

Persist bounded structured rejection categories in the durable ledger and
summarize exact counts in `recovery.log`. Any remaining unwritten item must be
shown to be unreadable from the source or intrinsically impossible to
represent safely, with comparative evidence and explicit human approval.
Qualify against the 19 GB source with exactly 37,402 readable candidates
reconciled to 37,402 unique written items, then require every output part to
pass independent readers, ScanPST, and Outlook.

The final current-code qualification completed in 9:30.47 at 323,200 KiB peak
RSS. It wrote all 37,402 candidates exactly once across five independently
validated parts, with no unsupported, stranded, duplicate, or unassigned item
and an unchanged source. The owner accepted this as completing version 0.4.4.

### 12. Version 0.4.5 - Direct-Write Performance

Make bounded direct output the default PST-writing execution mode for every
supported recovery policy. Stream recovered candidates through bounded
backpressure into the transactional PST writer,
write each part to temporary storage beside its final destination, and publish
by atomic rename after validation and `fsync`. Retain compact per-candidate and
part metadata for reporting and exact reconciliation, but do not create a
mailbox-sized payload spool or rewrite a finalized dataset.

Retain the existing durable ledger and payload spool behind an explicit
`--restartable` option. Only restartable mode accepts `--resume` or
`--keep-work`. An interrupted direct job is a reportable terminal partial
result; rerunning requires a new empty output directory. Report measured
source, temporary, validation, and finalized I/O so SSD write amplification is
observable. Preserve exact 0.4.4 data accounting, the
one-minute-per-source-GiB target, bounded memory, interruption safety, and
independent validity. First qualify the 19 GB source as one direct PST with
exact 1:1 content accounting and ScanPST/Outlook acceptance, then retain the
independently valid 4 GiB split regression.

### 13. Version 0.4.6 - Historical Corruption Recovery

Close the 0.4 correctness series against the external acceptance archive.
Treat every corrupt PST as an immutable source and its owner-supplied
ScanPST-repaired PST as an independent comparison reference. Recover every
pair that current libpff can read completely enough to compare into one direct
Unicode PST, require internal validation plus `pffinfo`, and compare a one-pass
semantic fingerprint covering placement, message classes, visible metadata,
bodies, recipients, recovery-critical properties, attachments, and recursive
embedded content. The repaired reference controls item multiplicity except for
complete, exact source-readable additions from a manifest-pinned category it
demonstrably drops or cannot read. Existing focused/full gates retain
independent `readpst`, empty-folder, raw-property, and named-property coverage;
do not repeat multi-gigabyte extraction for every historical pair.

The comparison manifest lives outside the repository and pins both files by
SHA-256. ScanPST-generated allocation identifiers and other structural
rewrites are not byte-equality requirements. Any semantic difference must be
reported by bounded category and resolved individually; committed evidence
must not contain mailbox values or private paths. Delete disposable recovered
outputs after retaining bounded conclusions. A source-readable supplement is
case-specific and category-pinned; it may only add complete values
demonstrably absent from the repaired reference while every other value remains
controlled by that reference. Retain and explicitly defer a compact pair only
when both source and repaired reference are unreadable in the required
category. The accepted result is 16 passing pairs, including 50 exact
source-proven associated additions, and three current-libpff cases reserved for
the post-1.0 fork.

### 14. Version 0.5.0 - Operational UX and Debian Packaging

Finalize human and JSON reports, documented exit codes, privacy-preserving
diagnostics, installation checks, and a reproducible Debian 13 x86_64 package.
Verify operation with Debian's older supported `libpff` ABI as well as the
newer Ubuntu development host package.

### 15. Version 0.5.1 - GitHub CI and Local Acceptance Policy

The GitHub remote and approved documentation baseline are available. Add branch
and pull-request checks, scheduled security and fuzzing jobs, and release
automation using only GitHub-hosted resources and public/generated inputs. Keep
the private corpus as an explicitly invoked local gate. Require ScanPST-first
human acceptance when a significant change can affect recovered content or
generated PST output, but not for changes that cannot affect output. Protect
`main` with required pull requests, hosted Ubuntu and Debian checks, and review
thread resolution.

### 16. Version 0.6.0 - Interoperability Release Candidate

Freeze the candidate CLI and schemas, run the complete external corpus,
exercise fault injection and resource limits, require clean ScanPST analysis,
and open and inspect generated parts in Outlook. Resolve all blocker and
high-severity adversarial review findings. MailPlus compatibility is assumed
after ScanPST and Outlook pass; MailPlus import is not a release gate.

### 17. Version 1.0.0 - MailPlus-Ready Release

Repeat the 50 GB recovery from a clean install, require clean ScanPST and
successful Outlook acceptance, reproduce the Debian package, verify licenses
and notices, finalize operator documentation, and record the release evidence.
A MailPlus-only failure after those checks is a Synology support issue and does
not block the release. Tag, publish, push, or merge only after explicit human
approval.

## Beyond 1.0

Post-1.0 candidates include selection by date range, partitioning by folder,
additional packing policies, EML, Maildir, PDF, other archival formats, and
broader platform packaging. They do not delay the native-item-to-PST recovery
release.
