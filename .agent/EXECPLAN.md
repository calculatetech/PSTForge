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
- [x] (2026-07-14) Milestone 0.1.0: Safe Foundation and Inspection. Added the
  Rust workspace, dynamically linked safe `libpff` boundary, read-only source
  identity and SHA-256 handling, `info` and basic `verify --mode full`,
  versioned human/JSON reports, structured logging, fake-backend tests, and
  local fast/full/release automation.
- [x] (2026-07-14) Milestone 0.1.1: Complete Mail Inventory.
  - [x] (2026-07-14) Added bounded folder/message traversal, owned known
    metadata, raw MAPI property streaming, recipients, attachment payloads,
    embedded-message relationships, unsupported-class accounting, and capped
    diagnostics through a safe candidate-event sink.
  - [x] (2026-07-14) Verified clean Unicode catalog invariants and a 64 KiB
    peak stream chunk on public external PSTs without source metadata or hash
    changes.
  - [x] (2026-07-14) Passed the complete fast gate plus license and RustSec
    checks; adversarial review capped diagnostic retention and removed the
    direct-message pending allocation.
  - [x] (2026-07-14) Passed the full gate on healthy ANSI and Unicode external
    PSTs, including source immutability plus independent `pffinfo` and
    `readpst` validation.
- [x] (2026-07-15) Milestone 0.2.0: Unicode PST Writer Foundation.
  - [x] (2026-07-14) Imported Microsoft `outlook-pst` 1.2.0 at pinned commit
    `1397836e73b690dbb09663f66056012fced45ff9`, retained its MIT license, and
    documented provenance in the separately licensed writer crate.
  - [x] (2026-07-14) Added template-free version 23 creation for a compact
    message store, name-to-ID map, root/IPM/deleted/mail folders, indexed
    hierarchy, contents, and associated-contents tables, and one plain-text
    message with an empty subnode tree.
  - [x] (2026-07-14) Passed internal header/map/BBT/NBT/heap/property/table/CRC
    and signature checks plus independent `pffinfo` and `readpst`; `readpst`
    extracted nonempty message data from the generated store.
  - [x] (2026-07-15) Imported the generated acceptance PST into a dedicated
    Synology MailPlus test mailbox and recorded folder/message counts before
    promotion. Candidate
    `cbdc012d420203348dad202cecefc133e6050e5a3f8addb19abd7199a18aca31`
    failed on 2026-07-15 with MailPlus `System error` and made Outlook terminate
    with resource exhaustion; it is rejected and must not be promoted.
    Corrected candidate
    `aa57b675b36e2d45833ba2891d85f8fbff9ff61a108e7af36d5e8a7ddce54ddc`
    imported but rendered trailing characters in the folder and subject, so it
    is also rejected for display fidelity. Candidate
    `bc82dfbc5ec6fe684d4799903b28ed89c16fa052a8687dd22ad8239ec5178312`
    reproduced both trailing characters and Outlook resource exhaustion on
    2026-07-15 and is rejected. Candidate
    `2ab18ffe2608895d38eb816424570b65728107f3e1f724335cae8c1c754f2009`
    passed the complete local and external-corpus gate and imported into
    MailPlus with exact folder/subject text, but Outlook still exhausted
    resources; it is rejected. Candidate
    `a68d52de1badaaba5225df300436cdc52035ef572a97577f51ab753da3c5f964`
    corrects the density-list/header BID mismatch but retains the broader r4
    Messaging-layer defects exposed by `scanpst`; it is superseded without
    promotion. Candidate
    `ffd03ea2c3cf195ce47f6962d3428e0fad3459e00dabf08acb94dcf34f466fec`
    incorporated the first `scanpst`-driven structural rewrite; its r6 log
    proved the NDB layer clean but exposed a row-ordering defect and missing
    Outlook maintenance objects, so it is superseded. Candidate
    `2bc7b7a75874bf4de5b046fcc2433a0b8e22e1729a238830c0a1f058e49804ca`
    matched the repaired r6 top-level node graph but retained four Messaging-
    layer schema/row diagnostics, so it is superseded. Candidate
    `14314f094fc636834fb78dca33f45a7bdea6e87b55551346b9c0eb6eeafaf118`
    corrected the r8 findings but retained the contents-row mismatch, so it is
    superseded. Candidate
    `0a26f87d3b2086d35a864f4cb39f26d25bc75f9edbdf2fada382af73b09b77d9`
    corrected the replication-property mapping but reproduced the same
    contents-row mismatch; it is rejected. The next candidate is blocked on a
    clean-context review and full gate after replacing heuristic message size
    with the checked serialized size. Candidate
    `4256ee7e02c60d8372e08719e2eb76f964004283d84db086ce086f58eabe9c7b`
    passed a clean-context adversarial review with no blocker/high findings and
    the current-source full gate. Its detailed `scanpst` log is clean with no
    errors, repairs, or recovered objects; the unchanged file opened normally in
    Outlook without resource exhaustion and imported perfectly into MailPlus.
- [x] Milestone 0.2.1: Mail-Fidelity PST Writer.
  - [x] (2026-07-15) Added a versioned canonical writer boundary for Unicode
    sender/subject/address values, To/Cc/Bcc recipient rows, Internet headers,
    text, HTML, literal-token LZFu RTF, by-value attachments, one-level
    embedded messages, deterministic named-property IDs, and supported raw
    properties.
  - [x] (2026-07-15) Corrected attachment message flags and attachment-local
    descriptor trees. `libpff` now accounts for two messages, four recipients,
    two attachments, and one embedded message with no issues; independent
    `readpst` emits the sender, both bodies, RTF, embedded RFC822 message, and
    byte-exact attachment marker.
  - [x] (2026-07-15) Extended the local writer gate to enforce those `libpff`
    counts and independent-reader markers from one deterministic rich fixture.
  - [x] (2026-07-15) Completed the first clean-context review remediation:
    property subnode/XBLOCK storage through 16 KiB, recursive validation and
    store-wide named IDs, arbitrary named GUID sets, additional scalar/GUID/
    fixed-width multivalue raw types, explicit omission reports, unique message
    record keys, honest RTF synchronization, inline attachment metadata, and
    message-class subclass validation.
  - [x] (2026-07-15) Full gate passed at `.agent/test-results/1784166700-full`:
    libpff reports two items, four recipients, two attachments, one embedded
    message, and no issues; pffinfo/readpst and all three external corpus cases
    pass; the complete 16 KiB attachment is decoded and byte-compared with its
    SHA-256 recorded in evidence.
  - [x] (2026-07-15) The next fresh review found malformed `MultipleGuid`
    encoding and incomplete omission/canonical-validation coverage. Added the
    initially added a count prefix in both property encoders, removed attacker-sized
    preallocation from its decoder, gave omissions deterministic message paths,
    exercised every advertised raw type, and made pre-publication validation
    recursively compare attachment metadata/content plus embedded message,
    recipient, named, raw, body, header, timestamp, and record-key values.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784167913-full`, including recursive typed internal
    comparison, clean libpff inventory, exact readpst attachment extraction,
    and all three external corpus reader pairs.
    This count-prefix decision was corrected on 2026-07-16 after ScanPST
    rejected the rich fixture: MS-PST PC storage uses packed fixed-width GUID
    elements and derives their count from the allocation length; the prefixed
    representation belongs to a different MAPI wire encoding.
  - [x] (2026-07-15) A further fresh review found that the typed comparison
    still did not prove absent optional values, top-level record keys,
    embedded RTF synchronization, attachment table identities, or NAMEID
    semantics. The validator now checks those relationships exactly, parses
    each numeric/string/GUID NAMEID identity, compares every raw NAMEID stream
    and hash bucket, and exercises two custom GUID sets across top-level and
    embedded messages. Production panic sites identified by review were also
    replaced with checked propagation or bounded conversions.
  - [x] (2026-07-15) Final-remediation full gate passed at
    `.agent/test-results/1784168632-full` with 29 PST crate tests, recursive
    typed/NAMEID validation, clean generated-store readers, and all external
    corpus cases.
  - [x] (2026-07-15) The next clean-context review found four remaining
    defensive-validation gaps. NAMEID string lookup now rejects malformed
    offsets without panicking; `rtf_in_sync` cannot be asserted without an RTF
    body; attachment-table size, number, filename, method, and rendering
    position are checked against both the source specification and attachment
    property context; and each supplied variable property is size-checked
    before writer-side allocation or cloning.
  - [x] (2026-07-15) Current-source full gate passed at
    `.agent/test-results/1784169113-full` with 30 PST crate tests, clean
    libpff/pffinfo/readpst acceptance, byte-exact 16 KiB attachment recovery,
    and all three external corpus reader pairs.
  - [x] (2026-07-15) A fresh review then found declared-length allocation in
    malformed NAMEID strings and missing preflight limits for generated
    recipient/NAMEID aggregates. NAMEID reads now consume only available bytes
    and reject truncated declarations; aggregate recipient metadata, each
    display-recipient property, every NAMEID stream, and collection counts are
    checked before construction. Boundary and malicious-length regression tests
    bring the PST crate total to 33.
  - [x] (2026-07-15) Post-review full gate passed at
    `.agent/test-results/1784169730-full` with clean generated-store readers and
    all external corpus cases.
  - [x] (2026-07-15) The following fresh review found prefix-tolerant malformed
    NAMEID parsing, aggregate checks that did not yet prove the single-page
    recipient and NAMEID encoders could succeed, and unbounded aggregate raw
    property amplification. NAMEID streams now require exact structure and
    zero padding; preflight uses encoder-compatible heap accounting; custom
    property count and aggregate payload are bounded before cloning; and public
    writer boundary tests exercise successful and rejected cases. Exact
    validation also covers SMTP/address duplicates, display-recipient
    properties, and long attachment filenames.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784170416-full` with 34 PST crate tests, clean
    generated-store readers, and every external corpus case.
  - [x] (2026-07-15) Another clean-context pass found untrusted multivalue
    counts/offsets that could request attacker-sized allocations, plus four
    fidelity/accounting gaps. Variable multivalue decoding now grows only from
    bytes actually read with checked offsets; minimal-store recipients are no
    longer discarded; bounded omission reports are constructed before atomic
    publication; NAMEID lookup accepts only parsed entry boundaries; and exact
    validation covers attachment-PC numbers, sender duplicates/address types,
    message flags, and has-attachments state.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784170998-full` with 36 PST crate tests, clean
    generated-store readers, and all external corpus cases.
  - [x] (2026-07-15) The next fresh review isolated two remaining malformed
    input cases: zero NAMEID bucket count and partial fixed-width multivalue
    tails. Bucket count zero is rejected before hashing, and all eight affected
    multivalue families now distinguish clean EOF from a truncated element.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784171319-full` with 38 PST crate tests and clean
    generated/external interoperability readers.
  - [x] (2026-07-15) A further fresh review found count-prefixed multivalue
    streams accepting undeclared trailing data and attachment readers
    materializing an entire untrusted local subnode tree. Count-prefixed
    decoders now require exact stream exhaustion. Attachment reading resolves
    method first, skips local subnodes for by-value data, and performs targeted
    embedded/storage lookup with cycle, depth, and cumulative-entry bounds.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784171831-full` with 40 PST crate tests and clean
    generated/external interoperability readers.
  - [x] (2026-07-15) The following fresh review traced legacy unbounded
    subnode and data-tree recursion below attachment property decoding, plus
    missing attachment-table and message-PC heap preflight. All subnode lookups
    now share targeted bounded traversal. Data-tree expansion is iterative with
    cycle/depth/entry and 64 MiB materialization limits plus exact declared-size
    reads. Writer validation preflights attachment tables, attachment PCs, and
    message PCs; boundary tests cover attachment/scalar overflow and empty
    by-value attachment fidelity.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784172625-full` with 41 PST crate tests and clean
    generated/external interoperability readers.
  - [x] (2026-07-15) The next clean-context review found recursive full-tree
    subnode enumeration, realizable multivalue count amplification, empty
    typed variable inputs that serialized as null, and attachment sizes that
    were trusted rather than checked. Subnode enumeration is now iterative and
    uses the same cycle/depth/entry budget as targeted lookup; count-prefixed
    variable multivalues have an explicit realizable element cap; empty
    required/optional variable writer inputs are rejected before publication;
    and attachment payload and object sizes are checked against the distinct
    data each field describes.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784173335-full` with 42 PST crate tests, clean
    generated-store libpff/pffinfo/readpst acceptance, exact attachment
    recovery, and all three external corpus reader pairs.
  - [x] (2026-07-15) The subsequent clean-context review found panic-prone
    corrupt BBT sizes, unbounded aggregate attachment-property materialization,
    missing delivery-time fidelity, and a pathname race between validation and
    publication. Block alignment and trailer arithmetic now reject malformed
    sizes through fallible production paths; attachment properties share a
    64 MiB cumulative materialization budget; delivery time is serialized and
    exactly checked in the message PC and contents row; and the writer validates
    through the held file descriptor before no-replace publication through held
    private-source and destination directory descriptors.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784174133-full` with 45 PST crate tests, clean
    generated-store independent readers, exact attachment recovery, and all
    external corpus cases. Regression tests exercise oversized BBT entries,
    aggregate property accounting, and publication after the visible private
    temporary-directory path is moved and replaced.
  - [x] (2026-07-15) The following clean-context review found stale-path
    cleanup risk after descriptor-based publication, success through a moved
    destination directory, a heap/table bypass around the data-tree byte cap,
    and no independent delivery-time assertion. Temporary cleanup guards are
    now disarmed immediately and file cleanup unlinks only through the held
    private directory; the requested path must
    resolve to the published inode before success; every materializing
    data-tree path enforces the 64 MiB ceiling and exact declared byte total;
    and libpff independently checks top-level and embedded delivery FILETIMEs.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784174888-full` with 46 PST crate tests, exact libpff
    delivery-time fidelity, clean generated-store independent readers, exact
    attachment recovery, and all external corpus cases. Publication tests also
    prove replacement content survives cleanup and a moved destination parent
    returns explicit uncertain-publication status.
  - [x] (2026-07-15) The next clean-context review found undersized and
    count-overflowing internal blocks that reached unchecked slice/arithmetic,
    plus stale pathname removal of an empty replacement directory. Internal
    block parsing now validates header space and uses checked entry and trailer
    arithmetic before allocation or slicing. Temporary file cleanup remains
    descriptor-only; pathname directory removal is deliberately forbidden, so
    an empty private `.pstforge-*` directory can remain after a write rather
    than risking deletion of unrelated replacement state. Later transactional
    job-directory cleanup may reclaim only identity-proven artifacts.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784175450-full` with 47 PST crate tests, exact libpff
    delivery times, clean independent readers and external corpus cases.
    Regression tests invoke production parsing for undersized and overflowing
    internal BBT entries and prove an empty stale-path replacement survives.
  - [x] (2026-07-15) A further clean-context review found that internal block
    logical size, trailer `cb`, and XBLOCK level/topology metadata could still
    disagree. Internal blocks now require exact header-plus-entry size and an
    identical trailer size. Data trees accept only level 1/2; level-2 entries
    must reference level-1 XBLOCKs, and level-1 entries must reference external
    data blocks. Production-path regressions cover size, level, child-kind, and
    child-level rejection.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784175782-full` with all 47 PST crate tests, exact
    libpff delivery times, clean independent readers, and all corpus cases.
  - [x] (2026-07-15) The next clean-context review found that lower-level
    XBLOCK construction did not yet enforce the exact invariants required by
    parsing. Constructors now reject header/entry-count disagreement, checked
    logical-size overflow, and trailer-size mismatch. A valid constructed
    XBLOCK is written and reread in the regression suite.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784176074-full` with all PST tests, exact libpff
    delivery times, clean generated-store independent readers, and every
    external corpus case.
  - [x] (2026-07-15) The following clean-context review found that attachment
    property accounting occurred after allocation, embedded-message properties
    did not share the budget, and XBLOCK child-kind checks were traversal-only.
    Property contexts now reserve external data-tree bytes before reading,
    charge decoded overhead, and share one 64 MiB budget across attachment and
    embedded-message property decoding. Unicode and ANSI XBLOCK construction
    and reading now reject level-1 internal children and level-2 external
    children; nonempty valid/invalid cases are tested.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784176561-full` with all 47 PST tests, exact libpff
    delivery times, clean rich embedded-message readers, and all corpus cases.
  - [x] (2026-07-15) The next clean-context review found four remaining
    resource and publication-order gaps. Store, folder, NAMEID, message,
    attachment, and embedded-message property reads now share mandatory 64 MiB
    materialization budgets; by-value attachment buffers use shared immutable
    storage; and storage payloads are charged before reading. Every child
    XBLOCK's `lcbTotal` is checked against its referenced leaf sizes. `pffinfo`
    and `readpst` now validate the held temporary file before no-clobber
    publication. The generalized empty NAMEID map preserves v0.2.0's reserved
    MAPI entry and bucket because libpff rejects zero-length required streams.
    Focused regressions cover cumulative budgets, child totals, and the empty
    NAMEID fallback; all 48 PST tests and warning-denied workspace clippy pass.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784177316-full` with all 48 PST tests, prepublication
    `pffinfo`/`readpst` acceptance, clean licenses and advisories, and all
    external corpus reader cases.
  - [x] (2026-07-15) The following clean-context review found no blocker/high
    issues and two medium acceptance gaps. Embedded-object validation now
    compares `PtypObject` size with an XBLOCK data tree's declared logical size
    instead of its physical root BBT size; an XBLOCK-root regression proves the
    distinction. The full gate's independent libpff sink now samples
    top-level and embedded NAMEID Unicode/integer/boolean values, raw
    Unicode/GUID properties, and the Bcc role/address. All 49 PST tests and the
    post-remediation full gate passed at
    `.agent/test-results/1784178107-full`; its `writer-acceptance.log` contains
    the expanded independent libpff evidence.
  - [x] (2026-07-15) A scope-filtered clean-context review found the preceding
    evidence reference stale and identified two directly applicable risks in
    the new synchronous prepublication validators. Each validator now runs in
    its own process group with a 60-second deadline; timeout kills the entire
    group. Stdout and stderr are continuously drained but retain at most 64
    KiB each. A rejection preserves the unpublished candidate and a synced
    bounded diagnostic in its private directory. Timeout, truncation, and
    retained-evidence regressions bring the PST crate suite to 51 tests.
  - [x] (2026-07-15) Post-remediation full gate passed at
    `.agent/test-results/1784178794-full` with 51 PST tests, warning-denied
    clippy, independent semantic writer acceptance, and all external corpus
    cases.
  - [x] (2026-07-16) Milestone-scoped review found one medium defect in the
    new validator deadline: a successful leader could exit while a descendant
    retained its pipes, leaving the diagnostic joins unbounded. Validator
    success now requires the leader and both pipe readers to finish before the
    deadline; otherwise the process group is killed and the candidate is
    retained as a timeout failure. The regression reproduces an immediate-exit
    shell with a background descendant.
  - [x] (2026-07-16) Post-remediation full gate passed at
    `.agent/test-results/1784179116-full` with all 51 PST tests, independent
    semantic writer acceptance, and all external corpus cases.
  - [x] (2026-07-16) Corrected ScanPST findings through clean-context reviewed
    r1-r3 candidates, added the normative compressed-RTF end reference for r4,
    declared UTF-8 HTML through `PidTagInternetCodepage` for r5, and added the
    explicit typed `PidTagNativeBody` selector for r6. Each code change passed a
    new clean-context adversarial review before further edits.
  - [x] (2026-07-16) Candidate r6 SHA-256
    `63a6b68e0cd6d44a1f5214032df6b5f49bc739d7d47d296e2f42528d4bdec88a`
    has a clean detailed ScanPST log, opens in Outlook without resource
    exhaustion, selects and renders exact non-ASCII HTML, and imports into
    MailPlus with exact HTML. Its generated-store gate passes 55 PST tests,
    formatting, check, warning-denied Clippy, documentation, artifact and diff
    checks, licenses, advisories, `libpff`, `pffinfo`, and `readpst`. The latest
    external-corpus phase was not rerun because `PSTFORGE_CORPUS_MANIFEST` was
    unset; the prior 0.2.1 external corpus gate remains clean.
- [x] (2026-07-16) Milestone 0.3.0: Recoverable Mail Pipeline.
  - [x] Added balanced normal-first `libpff` recovery enumeration for recovered
    and orphan collections with explicit provenance, recovery indices,
    nonzero stable-ID deduplication, and partial metadata/substream salvage.
  - [x] Added the private bundled-SQLite WAL ledger and content-addressed spool,
    one candidate transaction per message, crash-safe checkpoints, source
    identity binding, integrity/reopen validation, and strict private-state
    ownership, mode, link-count, hash, and size checks.
  - [x] Added `pstforge recover SOURCE --output JOB_DIR [--json]`, documented
    `0/1/3/4/6` status behavior, and retained the fresh-job/no-nonempty-output
    boundary. Importable size-limited PST parts remain milestone 0.4.0.
  - [x] Passed repeated clean-context adversarial reviews, the fast gate, the
    accepted external r6 fixture, and the real damaged Enron corpus case. The
    Enron run committed 2,178 candidates (2,173 complete and 5 partial),
    35,997 blobs totaling 15,198,391 bytes in 8.71 seconds with 20,672 KiB
    maximum RSS; source SHA-256 and timestamps remained unchanged.
- [ ] Milestone 0.3.1: Fault-Isolated Recovery.
- [ ] Milestone 0.4.0: Size-Limited PST Splitting.
- [ ] Milestone 0.4.1: Resume and 50 GB Qualification.
- [ ] Milestone 0.5.0: Operational UX and Debian Packaging.
- [ ] Milestone 0.5.1: GitHub CI and Private-Corpus Automation; the remote is
  reachable, and work begins after the approved baseline is pushed.
- [ ] Milestone 0.6.0: Interoperability Release Candidate.
- [ ] Milestone 1.0.0: MailPlus-Ready Release.

## Surprises & Discoveries

- Observation: An embedded-message attachment requires two distinct local
  descriptor levels: the parent message references the attachment PC, and the
  attachment PC's own descriptor tree references the embedded message PC and
  its recipient/attachment tables. Placing the embedded message directly in
  the parent descriptor tree lets simple readers see the attachment row but
  makes `libpff` reject the object lookup.
  Evidence: the generated 0.2.1 fixture initially reported one attachment and
  an invalid local-descriptor lookup; after the nested descriptor correction,
  `libpff` reported two attachments and one embedded message without issues.

- Observation: `PidTagHasAttachments` alone is insufficient for broad reader
  behavior. `PidTagMessageFlags` must also carry `MSGFLAG_HASATTACH`, and each
  attachment row needs a stable `PidTagAttachNumber`.
  Evidence: `readpst` extracted the by-value payload before `libpff` enumerated
  it; adding the conforming flags and numbering made both readers agree.

- Observation: Growing the deterministic 0.2.1 fixture past 16 KiB adds enough
  blocks to create a third BBT leaf. The internal reader and readpst traverse
  that candidate, but libpff 20231205 fails the embedded recipient descriptor.
  Evidence: a 32 KiB candidate reported three top-level recipients and one
  retained `stream recipients` issue; the 16 KiB bounded candidate reports all
  four recipients with no issues. Version 0.2.1 therefore rejects property
  payloads above 16 KiB before publication. Arbitrary BBT breadth and payload
  streaming remain required acceptance work for 0.4.x, not an implicit claim
  of this fixture writer.

- Observation: The vendored property-type conversion omitted `PtypObject`, and
  the attachment reader searched the parent message descriptor tree instead of
  the attachment-local tree before recursively opening an embedded message. It
  also attempted that recursive open while holding the PST reader mutex.
  Evidence: recursive pre-publication validation first saw only attachment
  properties preceding `PidTagAttachDataObject`, then found the embedded node
  missing, then deadlocked. Adding the Object mapping, reading the
  attachment-local descriptor tree, and dropping the reader lock before the
  recursive open makes the complete embedded message round-trip internally.

- Observation: Comparing only the value at a locally calculated named-property
  ID does not prove NAMEID identity. The GUID stream, entry stream, string
  offsets, and hash buckets can be wrong while the property value still appears
  at `0x8000 + index`.
  Evidence: the final validator now independently parses each completed entry
  and string/GUID identity and byte-compares all regenerated NAMEID streams and
  buckets; the fixture spans two custom GUID sets plus PS_MAPI.

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

- Observation: Ubuntu's packaged `cargo` and `rustc` do not install formatting
  or linting tools automatically. The verified additional package names are
  `rustfmt` and `rust-clippy`; RustSec checking requires `cargo install
  cargo-audit --locked`.
  Evidence: the initial 0.1.0 fast gate failed because `cargo fmt` was absent;
  the gate then passed with Ubuntu's 1.93 formatter and Clippy packages.

- Observation: The declared Rust 1.85 MSRV is sufficient for the complete
  0.1.0 workspace and locked dependencies.
  Evidence: `cargo check --workspace --all-targets --locked` passed with the
  official Rust 1.85.0 toolchain on 2026-07-14.

- Observation: A 31,761,408-byte public Enron Unicode PST provides a useful
  real-mail acceptance case without storing mail in the repository. PSTForge,
  `pffinfo`, and `readpst` all read it successfully; PSTForge accounted for 22
  folders and 2,178 reachable messages, and its SHA-256, inode, size,
  modification time, and access time remained unchanged across both commands.
  Evidence: the 0.1.0 full gate passed with the external manifest on
  2026-07-14; detailed local evidence remains under ignored
  `.agent/test-results/`.

- Observation: Deep enumeration shows the public Enron and larger Outlook
  fixtures contain broken recipient or attachment local descriptors even
  though shallow readers complete. They are useful damaged-continuation
  cases, but are not clean 0.1.1 acceptance fixtures. A separate public
  GroupDocs Unicode PST cleanly accounts for 43 folders, 10 messages, 10
  recipients, one attachment, 777 properties, and bounded 64 KiB chunks.
  Evidence: external catalog runs on 2026-07-14.

- Observation: Most available public vendor fixtures are Unicode. A 2009
  Govdocs1 file carrying a `.pst` extension is actually PostScript, and the
  attached Outlook 97-2002 sample located in an old support thread is no
  longer anonymously downloadable. Aspose Email 19.3 also rejects ANSI PST
  creation as unimplemented. Sourcegraph path search located msgvault's public
  65,536-byte Outlook version 14 store, which provided the clean ANSI case and
  passed PSTForge, `file`, `pffinfo`, and `readpst` classification/reading.
  Evidence: header classification, file inspection, and the 0.1.1 full gate on
  2026-07-14.

- Observation: `readpst` and Microsoft's reader accept an empty name-to-ID
  property context, but `libpff` refuses to open the store unless the entry and
  GUID streams contain data. A minimal MAPI named-property entry and PS_MAPI
  GUID stream satisfy all three readers without copying a template store.
  Evidence: rejected and accepted generated-store runs in the 0.2.0 full-gate
  evidence on 2026-07-14.

- Observation: Microsoft `outlook-pst` 1.2.0 holds the PST reader mutex while
  `root_hierarchy_table` opens a table context, which tries to acquire the same
  mutex and deadlocks. Microsoft's commit
  `d0f9f00110990f596ea6449c078640dc5bbf294e` fixes this after the pinned
  release; PSTForge backports that exact lock-scope correction and tests the
  path against every generated acceptance store.
  Evidence: the new writer round-trip test hung before the backport and passes
  after it on 2026-07-14; provenance is recorded in the writer crate's
  `UPSTREAM.md`.

- Observation: `pffinfo`, `readpst`, and Microsoft's adapted reader accepted a
  store whose root folder was its own NBT parent, whose advertised folder and
  message counts had no corresponding table rows, and whose density list
  overstated free allocation slots. MailPlus rejected that candidate with
  `System error`, while Outlook crashed with an out-of-memory or system-resource
  error consistent with cyclic traversal. The writer now emits an acyclic root,
  independent indexed hierarchy/contents tables, and matching AMap/DList free
  counts; tests traverse the complete folder/message path and cross-check the
  allocation metadata.
  Evidence: human MailPlus and Outlook smoke results reported 2026-07-15, known-
  good external table schemas, and the corrected writer structural tests.

- Observation: PST heap allocations for PtypString values are length-delimited
  in both property and table contexts. Known-good Outlook-compatible folder
  property and hierarchy-table allocations each store `Top of Personal
  Folders` as exactly 46 UTF-16LE bytes with no trailing null. PSTForge retained
  a null in the property context after correcting only the table path, so the
  third candidate reproduced MailPlus's `_` folder and `€` subject suffixes.
  Both serialization paths now share the exact-length encoding and regression
  coverage.
  Evidence: raw heap-allocation comparison against the external Aspose Unicode
  store and human MailPlus display results for the second and third candidates
  on 2026-07-15.

- Observation: The rejected generated stores left `bidUnused`, `dwUnique`, and
  every `rgnid` creation counter at zero/default values and marked density-list
  backfill incomplete. Microsoft-created stores use the required fixed
  `bidUnused` value, advance each node-type counter beyond emitted nodes, assign
  a nonzero unique value, and mark completed allocation metadata accordingly.
  The writer now emits those creation-state invariants, balances NBT leaf
  occupancy, and validates their raw header and density-list representation.
  Evidence: raw header/density-list comparison against Microsoft sample-header
  values and an Outlook-created external PST, plus writer regression tests on
  2026-07-15.

- Observation: The density-list page trailer BID MUST equal the header's
  `bidNextP`. Every Outlook-created and third-party control store satisfies this
  invariant, but all four rejected PSTForge candidates used the fixed density-
  list file offset `0x4200` as its trailer BID while the header advertised
  `0x104`. The adapted reader treats that mismatch as a missing or corrupt
  density list and initializes allocation-map backfill; Outlook's repeated
  resource exhaustion is consistent with taking that recovery path. The writer
  now uses the same next-page BID in both locations and tests the serialized
  equality.
  Evidence: raw header/trailer comparison against Outlook and Aspose control
  stores, adapted reader recovery logic, and the r4 Outlook result on
  2026-07-15.

- Observation: Outlook `scanpst` identified the complete r4 defect set rather
  than a single allocation-metadata error. The report found a zero-entry
  SLBLOCK, a non-null HNID for an empty Name-to-ID string stream, an unaligned
  heap page map, absent password/search/template/queue objects, incomplete
  hierarchy/contents/FAI schemas on every folder, and an unreadable contents
  table that made the folder's stored message count disagree with traversal.
  The writer now emits an aligned HN, a one-entry recipient subnode, null HNID
  for empty data, all six MS-PST 2.7 fixed templates, the search queues and
  minimum Search Root/spam hierarchy, complete `scanpst` column sets, and the
  required zero-valued PST password property. The specification-required root
  self-parent is restored; folder traversal uses hierarchy rows rather than NBT
  parent recursion.
  Evidence: `v0.2.0-mailplus-r4.log`, SHA-256
  `245c1c4cce87c3754383e44de5f6c843495a423897e7eeb32201a3e8598d3d3b`,
  Microsoft MS-PST 11.2 section 2.7, and expanded writer regression tests on
  2026-07-15.

- Observation: The r6 `scanpst` report contains no header, AMap, BBT, NBT,
  refcount, high-water-mark, heap, or low-level block errors. Its root hierarchy
  BTH was instead serialized in display order (`8022`, `8042`, `2223`) rather
  than ascending RowID order, which caused cascading missing-column, folder,
  and `PR_SUBFOLDERS` diagnoses. The writer now sorts every TC row set before
  emitting both its BTH leaf and row matrix. It also mirrors the repaired r6
  graph of 42 top-level nodes and 27 blocks: receive/outgoing tables, persisted
  index templates, search update/criteria/contents objects, SAL, and the
  reserved HMP. The message PC now carries the delivery time copied into its
  contents row. The repaired file retains ScanPST's synthetic `EC1`
  search-folder/update-queue warning, which is not treated as a PSTForge data
  loss defect.
  Evidence: `v0.2.0-mailplus-r6.log`, SHA-256
  `06e56855951c7fd1205e26601882c2f49bcac357d553172319e2a28a87d80340`,
  repaired r6 SHA-256
  `ff4d322ab9fac68b09ddee6e1e1ce04f18c31ce6615a258971415d920e83006f`,
  NBT/BBT and payload comparison, and full-gate evidence
  `.agent/test-results/1784153468-full` on 2026-07-15.

- Observation: The r8 report reduced ScanPST recovery to an absent empty-string
  default receive class, two incorrect outgoing-queue descriptors, one missing
  search-index descriptor, message instance metadata absent from the contents
  row, and the informational SDO deletion. The receive class is now an empty
  Unicode value with null HNID; the outgoing table uses `00390040` and
  `0E140003`; the search-index template includes `0E3E0102`; and the contents
  row carries the same `0E30`, `0E33`, and `0E34` instance metadata generated
  in the repaired r6 reference. Regression tests fix these tag/type/value
  contracts.
  Evidence: `v0.2.0-mailplus-r8.log`, SHA-256
  `19fde0ee65d030b47f017458d916b0dd408508bc511037efce0d0fb1bf56c767`,
  repaired-r6 row payload comparison, and full-gate evidence
  `.agent/test-results/1784154040-full` on 2026-07-15.

- Observation: The r9 and r10 reports both contain only the contents-row
  mismatch and ScanPST's informational SDO deletion. Correcting the replication
  property mapping did not change the diagnostic. A clean-context adversarial
  review then found that both the message PC and row used the same heuristic
  size (`325`) rather than the bytes consumed by the serialized message. The
  writer now computes one checked size from the PC plus recipient table (`556`
  for the compact fixture) and places it in both objects. The value differs from
  repaired r6's `558` because ScanPST retained one two-byte free heap-map entry.
  The same review found that publication lacked a pre-rename reopen/validation
  and parent-directory `fsync`; both are now required by the writer path.
  Evidence: `v0.2.0-mailplus-r9.log`, SHA-256
  `d1c51a1795f5159e783377fde75a2273de9d73cedbc8744795e4f6c155487a4f`,
  `v0.2.0-mailplus-r10.log`, SHA-256
  `d1c51a1795f5159e783377fde75a2273de9d73cedbc8744795e4f6c155487a4f`,
  repaired-r6 row/heap comparison, and clean-context adversarial review on
  2026-07-15.

- Observation: The post-r10 clean-context review found and resolved two writer
  publication defects unrelated to the ScanPST row: internal validation did not
  run before rename, and `tempfile::persist_noclobber` can fall back from atomic
  `renameat2(RENAME_NOREPLACE)` to link/unlink. The writer now reopens and checks
  the completed temporary store, uses safe `rustix` no-replace rename with no
  fallback, syncs the parent directory, and distinguishes a published output
  whose directory sync failed. A subsequent clean-context review found no
  blocker/high issue and authorized the full gate. The remaining medium test
  gaps now directly exercise the rename `EEXIST` branch, destination
  preservation, temporary-file retention, and the explicit published-but-
  durability-unknown outcome after a forced directory-sync failure.
  Evidence: focused writer tests and strict Clippy, clean-context reviews on
  2026-07-15, and full-gate evidence
  `.agent/test-results/1784160850-full`.

- Observation: ScanPST's r11 GUI reported the generic summary "minor
  inconsistencies" and offered optional repair, but its detailed log contains
  no `!!` error, no `??` repair, no orphan recovery, and no failed NDB or
  Messaging-layer phase. Running the optional repair can add synthetic Outlook
  maintenance state and produce a subsequent diagnostic, so the repaired copy
  is not evidence against the original. The byte-identical, unrepaired r11 PST
  is the only acceptance artifact; do not run repair before Outlook or MailPlus
  testing.
  Evidence: `v0.2.0-mailplus-r11.log`, SHA-256
  `29b6cd78feb3e2a91de8bc37392373cff71e0d623f9c409f0dc9ebd73062f398`,
  and r11 PST SHA-256
  `4256ee7e02c60d8372e08719e2eb76f964004283d84db086ce086f58eabe9c7b`
  on 2026-07-15.

- Observation: MailPlus converts a standards-compliant PST embedded-message
  attachment into a bare `message/rfc822` MIME entity, renders its text inline,
  and omits it from the ordinary-file attachment counter. A MailPlus-composed
  reference instead lists an attached email only by wrapping the complete
  exported MIME message as an `application/octet-stream` `.eml` file. The `.eml`
  itself still contains the original `message/rfc822` entity. Real-world PSTs
  containing attached messages are required before choosing whether PSTForge
  should offer a target-specific compatibility conversion.
  Evidence: `v0.2.1-fidelity-r5-mailplus.txt` and
  `v0.2.1-fidelity-mailplus-attachment.eml`, inspected 2026-07-16.

- Observation: MailPlus's attached-message presentation is not caused by the
  PSTForge writer. An equivalent message exported by Outlook and imported into
  MailPlus also omitted the embedded message from the visible attachment list,
  while its raw message retained the expected MIME content and exposed images
  from the nested message as separate attachments. Treat this as an external
  client behavior pending Synology guidance; do not convert MAPI embedded
  messages to opaque `.eml` files in PSTForge.
  Evidence: owner comparison using an Outlook-exported reference on 2026-07-16;
  no private message content is retained in repository evidence.

- Observation: Balanced recovery of the external damaged Enron PST completed
  2,173 messages and isolated five partial candidates without stopping the
  job. Its 31,761,408-byte source produced a 15,198,391-byte deduplicated spool
  in 8.71 seconds at 20,672 KiB maximum RSS, with identical source SHA-256,
  size, modification time, and access time before and after.
  Evidence: the ignored external-corpus recovery gate using the private
  manifest outside the repository on 2026-07-16.

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

- Decision: Version 0.1.0 `verify --mode full` reports only reachable folder
  and direct-message counts. Deleted/recovered and orphan counts serialize as
  `null` and display as `not scanned` until recovery enumeration is
  implemented; they must never be presented as zero merely because the scan
  was not run.
  Rationale: The first milestone establishes safe traversal, while the stable
  1.0 contract adds deeper property and recovery validation incrementally.
  Date/Author: 2026-07-14 / Codex.

- Decision: The source is opened once with Linux `O_NOFOLLOW`, `O_NOATIME`, and
  read-only flags, then libpff receives the held inode through
  `/proc/self/fd/<fd>` with its read access flag. Identity is checked before
  and after native parsing.
  Rationale: libpff's stable API accepts a filename rather than a Rust file
  descriptor. The proc descriptor path avoids returning to the user-controlled
  source path after the race-resistant open and preserves replaceable dynamic
  linking.
  Date/Author: 2026-07-14 / Codex.

- Decision: Version 0.1.1 exposes native catalog data as ordered events with
  owned metadata and borrowed payload slices no larger than 64 KiB. Direct
  messages and embedded messages use explicit work stacks, native strings are
  capped at 1 MiB, embedded depth at 64, and retained diagnostics at 10,000.
  Rationale: The future durable candidate spool needs complete property data
  without making memory proportional to a large body, attachment, or damage
  count. Explicit relationships avoid recursive native-handle ownership.
  Date/Author: 2026-07-14 / Codex.

- Decision: Inspection schema `1.1.0` adds recipient, attachment, embedded,
  unsupported, property, body, payload, peak-chunk, and omitted-issue fields.
  `verify` returns source-incomplete when either retained or omitted issues are
  nonzero.
  Rationale: Operators and corpus automation need uniform accounting and must
  not mistake a capped diagnostic list for a clean scan.
  Date/Author: 2026-07-14 / Codex.

- Decision: Version 0.2.0 emits a compact single-leaf BBT and a two-leaf NBT
  beneath an intermediate root. Folder and message discovery is represented
  consistently in both acyclic NBT parent identifiers and independent
  hierarchy/contents table rows; every folder also has an associated-contents
  node. The writer always serializes a nonempty minimal name-to-ID map for
  `libpff` interoperability.
  Rationale: The foundation must prove every PST layer and independent import
  without carrying opaque template bytes. Keeping the first graph within leaf
  capacities makes CRC, allocation, and relationship review tractable; leaf
  split boundaries and the serialized intermediate-root traversal are tested
  before the arbitrary-mail expansion.
  Date/Author: 2026-07-14 / Codex.

- Decision: Version 0.2.1 assigns named-property IDs by sorting GUID set and
  name, builds the name-to-ID streams and hash buckets per store, and rejects
  duplicate identities or raw properties that collide with writer-managed
  MAPI IDs. Embedded messages are limited to one attachment level at this
  writer boundary; unsupported deeper nesting is an explicit error.
  Rationale: deterministic assignment is required for byte-identical rebuilds,
  while silent collision or ambiguous recursive object graphs would lose
  source semantics. The later pipeline can mark unsupported content partial.
  Date/Author: 2026-07-15 / Codex.

- Decision: Version 0.2.1 derives a unique deterministic record key for each
  top-level or embedded message, treats RTF synchronization and inline
  attachment metadata as explicit inputs, assigns named properties across the
  complete message graph, and returns structured accounting for unsupported
  properties. Its canonical single-message property payload limit is 16 KiB;
  exceeding it is a checked error before the temporary file is published.
  Rationale: fidelity metadata must not be fabricated or silently dropped, and
  a bounded, independently verified writer is preferable to producing a wider
  BBT layout that a required reader cannot traverse. The 0.4.x packer owns the
  arbitrary-size BBT and multi-part design.
  Date/Author: 2026-07-15 / Codex.

- Decision: Version 0.2.1 defines binary HTML body input as UTF-8, rejects
  invalid UTF-8 before creating the temporary store, and emits
  `PidTagInternetCodepage` (`0x3FDE`) with value 65001 on every top-level and
  embedded message. It does not synthesize `PidTagMessageCodepage` (`0x3FFD`),
  whose generic fallback is redundant when the explicit Internet code page is
  present; a source `0x3FFD` supplied through the typed raw-property boundary
  remains preservable. The IDs remain distinct in validation: `0x3FDE` is
  writer-managed, while `0x3FFD` is absent from PSTForge's canonical fixture.
  Rationale: Microsoft requires the Internet code page when binary HTML is
  present. Without it, MailPlus guessed UTF-16LE and paired the fixture's UTF-8
  bytes into mojibake even though Outlook rendered the same property. Fixing
  the encoding contract preserves byte-exact non-ASCII HTML across clients
  without expanding this milestone into arbitrary legacy-code-page support.
  Date/Author: 2026-07-16 / Codex.

- Decision: Version 0.2.1 represents `PidTagNativeBody` (`0x1016`) as an
  optional typed input with plain-text, RTF, and HTML variants. A selected
  representation must be present; the writer does not derive a preference from
  which bodies happen to exist. The canonical top-level fixture selects HTML,
  and its embedded message selects plain text. Raw-property input cannot
  override this writer-managed semantic.
  Rationale: Multiple body representations can coexist without identifying
  which one is authoritative. Candidate r5 preserved every body and produced
  byte-exact UTF-8 HTML, but Outlook selected RTF because the best body was
  unspecified. An explicit optional selector preserves source absence while
  allowing deterministic client body selection without fabricating fidelity.
  Date/Author: 2026-07-16 / Codex.

- Decision: Version 0.2.1 preserves embedded messages as MAPI
  `afEmbeddedMessage` attachments and does not silently replace them with
  by-value `.eml` files. A MailPlus compatibility transformation is deferred
  until external corpus cases demonstrate how genuine PST embedded messages
  import and whether conversion improves accessibility without losing source
  semantics. The canonical fixture's Outlook list preview may show its RTF
  checkpoint while the opened message shows native HTML because its three body
  fixtures deliberately differ and `rtf_in_sync` is false; that synthetic
  preview difference is not a release defect.
  Rationale: Microsoft requires method-5 embedded messages to convert to
  `message/rfc822`; the current raw export proves that behavior. The MailPlus
  reference uses an opaque outer `.eml` wrapper, which is a target-specific
  transformation rather than a metadata correction. Guessing that policy from
  one synthetic case would reduce fidelity and require a MIME serialization
  contract without representative source evidence.
  Date/Author: 2026-07-16 / Codex.

- Decision: Version 0.3.0 exposes a balanced `recover` command that creates a
  durable canonical job but does not claim to create importable PST parts.
  Normal mail is processed before `libpff_file_recover_items` with flags zero,
  followed by recovered and orphan collections. Aggressive allocation-ignore
  and fragment flags remain version 0.3.1.
  Rationale: operators and the external corpus need an executable recovery
  checkpoint now, while supervised native workers and PST packing have their
  own milestone acceptance boundaries.
  Date/Author: 2026-07-16 / Codex.

- Decision: The 0.3.0 job uses bundled SQLite in WAL/FULL-synchronous mode and
  a private SHA-256 spool. One message is one immediate transaction; complete
  properties and attachments have explicit end events, recoverable stream
  failures have explicit abort/partial markers, and sink or resource-limit
  errors remain fatal. Job, SQLite, sidecar, and blob state must be owned by
  the effective user, inaccessible to group/other, and not hard linked.
  Rationale: committed candidates must survive process termination without
  silently upgrading partial data or exposing mailbox content.
  Date/Author: 2026-07-16 / Codex.

- Decision: Concurrent hostile mutation by another process with the same UID
  is outside the 0.3.0 filesystem threat boundary. SQLite cannot combine its
  `SQLITE_OPEN_NOFOLLOW` flag with the held `/proc/self/fd/<dir>` path used for
  job containment; pre/post inode, type, owner, permission, and link checks
  remain mandatory. Untrusted PST bytes never control filesystem operations.
  Rationale: the no-follow flag rejects the proc-descriptor parent and makes
  every legitimate ledger open fail. A same-UID attacker can already rewrite
  the owner's private job state and requires OS isolation beyond this tool.
  Date/Author: 2026-07-16 / Codex.

## Outcomes & Retrospective

Version 0.1.0 now lets an operator inspect a healthy PST in human or JSON form
and inventory its reachable folders/messages without changing the source. The
workspace compiles at the Rust 1.85 MSRV, links dynamically to libpff 20231205,
and passed formatting, check, Clippy, unit tests, rustdoc warnings, schema and
documentation checks, license policy, RustSec audit, a real-mail external
corpus run, `pffinfo`, and `readpst`. Recovery-only enumeration, message
properties, attachment streaming, damaged-record continuation, and the PST
writer remain later milestones. The main lesson is that recovery counters must
encode "not scanned" separately from zero and that privacy-safe automation
must suppress independent-reader output because it can contain mailbox names.

Version 0.1.1 extends full verification to the complete reachable mail
catalog. It owns metadata before releasing native handles, streams raw
properties and attachments in at most 64 KiB chunks, records recipients and
embedded relationships, identifies unsupported classes, and caps depth,
counts, strings, and diagnostics. The full gate passed clean ANSI and Unicode
external PSTs, including a Unicode attachment/body case, with source hashes,
identity metadata, and access/modify times unchanged. `pffinfo` and `readpst`
also accepted every promoted corpus case. Damaged-item recovery and
fault-isolated continuation remain 0.3.x work rather than being silently
claimed by this healthy-inventory milestone.

Version 0.2.0 reached its human interoperability gate with unrepaired candidate
r11: its detailed ScanPST log is clean, Outlook opens it without resource
exhaustion, and MailPlus imports the exact folder and message. Earlier, the
first candidate was rejected after
MailPlus reported `System error` and Outlook exhausted resources; no commit or
merge was made. A second candidate imported but failed exact folder/subject
display fidelity. A third candidate retained the same property-context string
defect and still exhausted Outlook resources, exposing invalid initial header
counters and allocation backfill state. The corrected adapted MIT crate creates
a Unicode v23 store from
typed structures, not a template, with compressible permutation encoding,
valid header and page CRCs, block signatures, consistent allocation maps and
density lists, leaf BBT/NBT, indexed property/table heaps, required folders,
and one plain-text message. No milestone commit or merge is permitted until
MailPlus confirms the folder and message import without Outlook resource
  failure. The post-acceptance clean-context review found no unresolved blocker,
  high, or medium issue, and the final current-source full gate passed with both
  required healthy ANSI and Unicode external cases.

Version 0.2.1 now writes deterministic rich mail with typed sender, recipient,
timestamp, plain-text, UTF-8 HTML, compressed RTF, native-body, Internet-header,
by-value attachment, embedded-message, named-property, and raw-property
semantics. Candidate r6 is ScanPST-clean, opens without Outlook resource
exhaustion, renders its declared native HTML in Outlook and MailPlus, and passes
all generated-store independent readers. MailPlus's presentation of true
embedded messages remains a documented external-corpus question rather than a
reason to replace standards-compliant source semantics speculatively.

Version 0.3.0 now gives an operator a balanced, source-preserving recovery
command and a durable private job containing normal, recovered, and orphan
mail candidates. It does not yet produce the smaller PST files needed for
MailPlus import. The real damaged Enron corpus demonstrated candidate-level
containment and low memory use: 2,178 candidates committed, five partial, no
source identity change, 8.71 seconds elapsed, and 20,672 KiB maximum RSS. The
next milestone adds native crash isolation and bounded retries before 0.4.0
packs the accepted spool into importable parts.

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
    pstforge recover <source.pst> --output <job-dir> [--json]
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
