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
unrelated mail from reaching the durable recovery spool.

### 6. Version 0.3.1 - Fault-Isolated Recovery

Run native parsing in supervised workers, checkpoint after bounded units of
work, contain crashes, and retry or isolate failing items without losing prior
progress. Add aggressive recovery that scans fragments and disregards suspect
allocation metadata while keeping balanced recovery as the default.

### 7. Version 0.4.0 - Size-Limited PST Splitting

Implement deterministic packing into independently valid PST parts with a
user-configurable maximum and a 4 GiB default. Replicate the required folder
hierarchy in each part, assign every writable message exactly once, validate
before atomic publication, and preserve an indivisible oversize message in a
clearly marked oversize part.

### 8. Version 0.4.1 - Resume and 50 GB Qualification

Add a transactional job ledger, private recovery spool, explicit compatible
resume, graceful interruption, progress, disk-space checks, and cleanup. Run
the real 50 GB corrupt PST end to end and show that forced termination cannot
damage the source or already finalized parts.

### 9. Version 0.5.0 - Operational UX and Debian Packaging

Finalize human and JSON reports, documented exit codes, privacy-preserving
diagnostics, installation checks, and a reproducible Debian 13 x86_64 package.
Verify operation with Debian's older supported `libpff` ABI as well as the
newer Ubuntu development host package.

### 10. Version 0.5.1 - GitHub CI and Private-Corpus Automation

The GitHub remote and approved documentation baseline are available. Add branch
checks, scheduled security and fuzzing jobs, release automation, and an
explicitly invoked private self-hosted corpus runner that never uploads PST
content or sensitive logs. This repository does not use pull requests, so
required checks apply to milestone branches and pre-merge local gates.

### 11. Version 0.6.0 - Interoperability Release Candidate

Freeze the candidate CLI and schemas, run the complete external corpus,
exercise fault injection and resource limits, import generated parts into a
test Synology MailPlus mailbox, and perform secondary Outlook checks. Resolve
all blocker and high-severity adversarial review findings.

### 12. Version 1.0.0 - MailPlus-Ready Release

Repeat the 50 GB recovery and MailPlus import rehearsal from a clean install,
reproduce the Debian package, verify licenses and notices, finalize operator
documentation, and record the release evidence. Tag, publish, push, or merge
only after explicit human approval.

## Beyond 1.0

Post-1.0 candidates include selection by date range, partitioning by folder,
additional packing policies, contacts and calendar items, EML, Maildir, PDF,
other archival formats, and broader platform packaging. They do not delay the
mail-to-PST recovery release.
