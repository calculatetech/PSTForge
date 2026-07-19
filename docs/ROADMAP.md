# PSTForge Roadmap to 1.0

## Goal

PSTForge 1.0 will run on Linux, read a damaged Microsoft Outlook PST without
modifying it, recover as much mail as can be handled safely, and write smaller
independently valid PST files for import into Synology MailPlus Server.
Microsoft Outlook is a secondary interoperability target.

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
the 5,317,328,896-byte result as a known limitation, while the 2 GiB objective
remains for later optimization and release-scale qualification. Content
fidelity findings discovered during qualification are planned separately as
version 0.4.2 and are not folded into the accepted splitting branch.

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

Restore the 19 GB cold-run ceiling to 20 minutes on the current host, target the
previously demonstrated approximately ten-minute result, publish the first
part within six minutes, and keep aggregate PSTForge RSS below 2 GiB. Complete
the deferred 0.4.2 whole-job reconciliation across independently validated
4 GiB splits before proceeding. A low-write direct-output mode remains future
work after the newly exposed native-item data-correctness gaps are resolved.

### 11. Version 0.5.0 - Operational UX and Debian Packaging

Finalize human and JSON reports, documented exit codes, privacy-preserving
diagnostics, installation checks, and a reproducible Debian 13 x86_64 package.
Verify operation with Debian's older supported `libpff` ABI as well as the
newer Ubuntu development host package.

### 12. Version 0.5.1 - GitHub CI and Private-Corpus Automation

The GitHub remote and approved documentation baseline are available. Add branch
checks, scheduled security and fuzzing jobs, release automation, and an
explicitly invoked private self-hosted corpus runner that never uploads PST
content or sensitive logs. This repository does not use pull requests, so
required checks apply to milestone branches and pre-merge local gates.

### 13. Version 0.6.0 - Interoperability Release Candidate

Freeze the candidate CLI and schemas, run the complete external corpus,
exercise fault injection and resource limits, import generated parts into a
test Synology MailPlus mailbox, and perform secondary Outlook checks. Resolve
all blocker and high-severity adversarial review findings.

### 14. Version 1.0.0 - MailPlus-Ready Release

Repeat the 50 GB recovery and MailPlus import rehearsal from a clean install,
reproduce the Debian package, verify licenses and notices, finalize operator
documentation, and record the release evidence. Tag, publish, push, or merge
only after explicit human approval.

## Beyond 1.0

Post-1.0 candidates include selection by date range, partitioning by folder,
additional packing policies, contacts and calendar items, EML, Maildir, PDF,
other archival formats, and broader platform packaging. They do not delay the
mail-to-PST recovery release.
