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
- [x] (2026-07-16) Milestone 0.3.1: Fault-Isolated Recovery.
  - [x] Moved native parsing behind a versioned, bounded, source-identity-checked
    worker protocol with a supervisor watchdog, three retries, durable replay,
    worker-event accounting, and exact isolated-unit records.
  - [x] Made folders and top-level messages independently addressable through
    stable bounded child-index paths; candidates retain their recovery-unit
    identity so an isolated unit can keep prior committed mail without causing
    replay-order cascades.
  - [x] Added explicit balanced/aggressive recovery selection. Aggressive sets
    both libpff allocation-ignore and fragment-scan flags while generic
    recovered-list results retain `recovered` provenance; current upstream
    libpff exposes no constructed fragment objects, so fragment totals remain
    zero rather than overstating recovery.
  - [x] Passed workspace tests and external real-PST checks for transient and
    persistent aborts, stalls, folder/unit isolation, late in-unit failure
    after a durable candidate, aggressive accounting, and source immutability.
  - [x] Added graceful SIGINT/SIGTERM handling that kills the active worker,
    rolls back only the active candidate, checkpoints prior commits, records
    interruption, and returns a durable partial report. Added a real SIGSEGV
    shim and a parser-error-after-commit shim; both pass external real-PST
    replay/continuation checks without changing the source.
  - [x] Completed clean-context adversarial review and remediated late
    in-unit replay isolation, folder/child-pointer isolation, truthful
    aggressive provenance, and exit-130 signal handling. The remaining review
    suggestion to create durable state during the initial worker handshake was
    rejected as outside the documented contract: no durable checkpoint has
    yet been attempted, and invalid/unopenable input must not leave a job.
  - [x] Passed `cargo xtask gate full`, including licenses, advisories, writer
    interoperability, and the external corpus. Evidence:
    `.agent/test-results/1784216979-full`.
- [x] Milestone 0.4.0: Size-Limited PST Splitting.
  - [x] (2026-07-16) Added deterministic canonical translation, ordering, and
    packing from the durable recovery spool. The packer estimates store,
    folder, message, property, and attachment cost, then enforces the requested
    maximum against the completed PST and rebuilds an over-target normal part
    before publication. A single indivisible over-target message is retained
    in a marked oversize part with partial-success accounting.
  - [x] (2026-07-16) Extended the writer to arbitrary multi-folder and
    multi-message stores, recursive NBT/BBT and descriptor trees, streamed
    property/attachment blocks, source creation/modification timestamps, and
    deterministic per-part store identity derived from source SHA-256,
    recovery mode, maximum size, writer format, and part index.
  - [x] (2026-07-16) Added publication intents, private sibling scratch state,
    validation, `fsync`, atomic PST and sidecar publication, exact typed
    sidecar-to-ledger binding, transactional item assignment, and crash
    reconciliation. Reopen rejects missing, extra, symlinked, replaced,
    malformed, or unledgered part artifacts.
  - [x] (2026-07-16) Added boundary, table-growth, hard-limit, oversize,
    deterministic packing, property fallback, invalid FILETIME, message-class,
    publication-crash, artifact-tamper, and source-immutability tests. The real
    public GroupDocs Unicode PST splits at 2 MiB into two byte-identical runs,
    with five messages per part and no source identity change.
  - [x] (2026-07-16) Strengthened the external gate to independently stream
    libpff fingerprints for every source/output message and compare exact
    multiplicity, source folder placement, sender, subject, all four
    timestamps, body properties, recipients, attachment payloads, and
    MIME/inline rendering metadata. It independently derives leaf and hierarchy
    folder counts, reads the actual store record key, and compares deterministic
    sidecar bytes across runs.
  - [x] (2026-07-16) Completed clean-context adversarial remediation for hard
    size enforcement, publication ordering, stale-path cleanup, validator-log
    privacy, exact sidecar binding, folder/message fidelity, and false-pass
    risks. The final property-containment remediation bounds and validates
    streamed UTF-16 and compressed RTF before writing, preserves valid report
    classes, and makes every unsupported source property an explicit partial
    omission. It also preserves source message flags and Internet codepages,
    bounds attachment streaming to declared size, retains empty attachments,
    and makes unavailable/oversize attachment content candidate-local partial
    damage. Final focused reviews also added a side-effect-free writer input
    preflight, candidate-local bisection for unrepresentable mail, explicit
    unsupported accounting for deeper embedded descendants, and kept writer
    construction/self-validation failures fatal. Attachment recovery now
    continues after a damaged index, incomplete payload prefixes remain private,
    and canonical replay binds attachment type, terminal state, declaration,
    payload, embedded ownership, and property owners before publication. A
    supported parent remains writable when its embedded child has an unsupported
    message class, with that attachment explicitly omitted. Resume numbering and
    stale partial cleanup findings were confirmed as version 0.4.1 scope.
  - [x] (2026-07-16) Passed the final accepted-source full gate, including formatting,
    warning-denied compilation and Clippy, workspace tests, documentation,
    artifacts, licenses, RustSec advisories, writer `pffinfo`/`readpst`, writer
    acceptance, all external corpus cases, and the deterministic real-PST split
    gate. Evidence: `.agent/test-results/1784248198-full`.
  - [x] (2026-07-16) Remediated the independent Microsoft ScanPST findings from
    split candidates r1 through r4: data-tree totals and full intermediate HN
    pages, BBT reference counts, bounded HN allocations and fill maps, copied
    contents-table properties, and folder/hierarchy unread counts. Malformed or
    oversized copied properties are now candidate-local explicit omissions,
    not reasons to discard an otherwise recoverable message. Candidate r5
    retains five messages in each part. The owner reports that ScanPST found no
    errors and both parts open successfully in Outlook; no r5 ScanPST log was
    copied into the external acceptance directory, so this conclusion relies
    on the reported human run.
  - [x] (2026-07-16) Imported both r5 parts into MailPlus. Part 0001 contains one
    message in Deleted Items and four Inbox messages, two unread; part 0002
    contains five read Inbox messages. MailPlus imported the nine Inbox
    messages into one Inbox but did not import the Deleted Items message. The
    generated PST still contains and exposes that deleted-folder message, so
    the missing MailPlus item is recorded as target importer behavior rather
    than PSTForge loss.
  - [x] (2026-07-16) Opened both r5 split parts in Outlook after ScanPST found
    no errors. Part 0001 exposes one Deleted Items message and four Inbox
    messages, two unread; part 0002 exposes five read Inbox messages. Outlook
    opened both independently without the prior crash or resource failure.
  - [x] (2026-07-16) The final whole-milestone review found that native message
    identifiers were incorrectly deduplicated globally even though embedded
    message NIDs are scoped to their attachment subnode tree. Deduplication now
    applies only to nonzero top-level identifiers; embedded ownership remains
    keyed by its durable parent candidate and attachment path. A focused
    regression forces the same identifier through top-level and embedded
    scopes. A fresh clean-context remediation review returned `CLEAN`.
- [x] Milestone 0.4.1: Resume and 50 GB Qualification.
  - [x] (2026-07-16) Implemented exact read-only resume validation for source
    identity and SHA-256, tool major, schema, recovery mode, hard size policy,
    and writer format. Completed jobs reuse their durable recovery completion
    without restarting libpff; historical spool metrics and finalized part
    hashes remain stable across repeated resume.
  - [x] (2026-07-16) Added `split --resume` and `--keep-work`, continued part
    numbering, automatic ledger integrity checks, stale partial and writer
    scratch cleanup, three-times-source disk preflight, privacy-safe progress,
    elapsed/throughput/output metrics, and sampled supervisor/worker peak RSS.
  - [x] (2026-07-16) Added real-PST gates for completed resume, immutable
    mismatch refusal, SIGTERM checkpoint/resume, SIGKILL recovery, worker
    parent-death containment, source immutability, and finalized-part
    preservation. Focused external corpus runs pass.
  - [x] (2026-07-16) Passed the full automated gate, including formatting,
    warning-denied compilation and Clippy, workspace tests, documentation,
    artifacts, licenses, RustSec advisories, writer acceptance, all external
    corpus tests, and independent `pffinfo`/`readpst` checks. Evidence:
    `.agent/test-results/1784253706-full`.
  - [x] (2026-07-16) The first milestone-wide clean-context review found one
    high and two medium resume risks. Remediation credits only validated
    existing job allocation against resume capacity, revalidates immutable
    metadata on the exact held ledger before reconciliation, and observes
    interruption after hashing, after each atomic publication, and around
    final cleanup. The real-PST signal gate now interrupts after part 0001 is
    finalized and proves resume preserves its hash and continues numbering.
  - [x] (2026-07-16) The next fresh review found three medium scale-workflow
    gaps. Remediation installs signal handling before source hashing and checks
    it at each 1 MiB source/blob/part hash chunk, rejects untracked private-root
    entries before allocation credit, and measures an existing job directory
    itself so a job mountpoint uses the correct filesystem. A direct 2 GiB
    pre-hash SIGTERM trial exited 130 without creating a job; focused tests and
    the full gate pass.
  - [x] (2026-07-16) The third fresh review found three remaining medium gaps.
    Remediation threads interruption through recovery source rechecks, staged
    part hashes, publication verification, and publication-intent
    reconciliation; rejects every untracked spool entry before allocation
    credit; and opens a resume ledger query-only until exact immutable metadata
    and integrity are validated, enabling WAL/configuration only afterward.
    Focused regressions and the full gate pass.
  - [x] (2026-07-16) The fourth fresh review found one remaining medium
    interruption pattern. Standalone recovery now installs its handler before
    source hashing, worker retry and every ordinary split ledger reopen use
    interruptible validation, and an already-interrupted report skips
    secondary full-job hashes. Focused real-PST tests and the full gate pass.
  - [x] (2026-07-16) The fifth fresh review found one relevant medium capacity
    issue: retained independent-validator failure scratch could be credited as
    resumable job allocation. Resume capacity now credits the private ledger,
    validated spool, and finalized parts but excludes all contents beneath the
    diagnostic partial directory. A regression retains the diagnostic evidence
    while proving its failed PST allocation is not credited.
  - [x] (2026-07-17) Diagnosed the first 19 GB qualification attempt without
    modifying its source or job. After about twelve minutes the old path had
    created 72,905 spool files, written about 26 GB physically, and performed
    about 1.5 TB of logical reads without publishing a part. SIGTERM could not
    complete promptly inside the synchronous hot path; the owner used SIGKILL,
    and the parser child exited through its parent-death signal. The durable
    schema-5 job remains available for compatible resume.
  - [x] (2026-07-17) Removed the observed scale amplification while retaining
    streaming and crash containment. Properties through 64 KiB use
    SHA-verified SQLite BLOB storage; larger values remain durable streamed
    spool files. Candidate work commits in batches of at most 128 with a
    per-candidate savepoint, so graceful interruption commits completed
    candidates and abrupt process death can replay only the current bounded
    batch. Existing schema-5 file-spool jobs gain the additive table and
    blob-reference index on reopen.
  - [x] (2026-07-17) Prefiltered candidate conversion before packing so one
    unrepresentable message cannot repeatedly rebuild or fragment otherwise
    valid parts. A fresh 2,178-message, 31,761,408-byte real-PST benchmark
    completed in 7.83 seconds with 202,296 KiB maximum RSS, one 25,920,512-byte
    part, 2,147 written candidates, 31 explicitly unsupported candidates, and
    21 large spool files. The pre-remediation binary was killed after 334.77
    seconds with 80 fragmented parts and 35,997 spool files. Reopening and
    validating the completed job fell from more than 103 seconds without
    completion to 1.93 seconds after indexing blob references.
  - [x] (2026-07-17) The first updated full gate caught a worker-crash
    lifecycle regression: the retry branch dropped an open candidate batch and
    replayed completed work. It now aborts only the active candidate and commits
    the completed bounded batch before reopening the ledger. The focused
    persistent-abort real-PST regression again passes with four bounded worker
    failures, one surviving committed candidate, and explicit partial status.
  - [x] (2026-07-17) The refreshed full milestone gate passes formatting,
    warning-denied compilation and Clippy, all workspace and external-corpus
    tests, documentation and artifact checks, licensing, RustSec advisories,
    writer acceptance, and independent `pffinfo`/`readpst` checks. Evidence:
    `.agent/test-results/1784264675-full`.
  - [x] (2026-07-17) A fresh scale-focused review found one high and three
    medium issues, all relevant to 0.4.1. Remediation keeps `readpst` extraction
    beneath the held private publication directory; securely deletes inline
    payloads, checkpoints WAL, and vacuums the retained ledger; additively
    indexes candidate occurrence lookup in schema 5; and makes PST block/message
    construction plus validator process groups interruption-aware. Query-plan,
    private-sentinel/compaction, scratch-boundary, validator-process, and
    failure-classification regressions pass.
  - [x] (2026-07-17) The representative no-`--keep-work` benchmark, including
    secure ledger compaction, completed in 8.14 seconds with 201,840 KiB
    maximum RSS, one 25,920,512-byte part, zero retained spool files, and a
    54 MiB completed job.
  - [x] (2026-07-17) The post-review full milestone gate passes all local,
    external-corpus, license, advisory, writer acceptance, and independent
    reader checks. Evidence: `.agent/test-results/1784265413-full`.
  - [x] (2026-07-17) The next fresh review found one remaining medium issue:
    schema migration, integrity SQL, secure deletion, and compaction could
    suppress SIGTERM for a large ledger. SQLite work now runs with a monitoring
    thread and interrupt handle, filesystem cleanup checks the same flag, and a
    durable `cleanup_compaction_pending` marker makes interrupted vacuum
    restartable. The external real-PST gate waits for deliberately long cleanup
    SQL, sends SIGTERM, requires status 130, and proves resume finishes cleanup.
    Crash-left nested `readpst` extraction is safely removed through held
    directory handles.
  - [x] (2026-07-17) The cleanup-interruption remediation passes the full
    milestone gate, including the expanded external signal/resume case.
    Evidence: `.agent/test-results/1784266394-full`.
  - [x] (2026-07-17) A fresh review found two remaining medium containment
    gaps relevant to the qualification run: canonical catalog reconstruction
    and candidate prefilter translation did not observe SIGTERM throughout,
    and a supervisor killed during independent validation could leave the
    validator process tree alive. Catalog, event, ownership, blob-verification,
    translation, and prefilter paths now share the interruption flag. Validators
    run through a hidden parent-death supervisor that owns their process group
    and kills the complete group when the PSTForge parent disappears.
  - [x] (2026-07-17) Added focused regressions that interrupt at candidate
    prefilter and resume from the durable job, and that prove a stalled
    validator plus descendant cannot outlive its parent. Reconciled
    `Cargo.lock` offline after moving runtime signal/process support into the
    CLI dependency set; `cargo check --locked` and the fast gate pass.
    Evidence: `.agent/test-results/1784268053-fast`.
  - [x] (2026-07-17) The first fresh optimized 19 GB run proved the SQLite
    inline-blob remediation was still the wrong large-mailbox architecture.
    Recovery completed in about 4 minutes 40 seconds, but 37,373 candidates,
    3,616,287 event rows, and 714,994 inline blobs then entered a silent
    per-candidate replay. After 14 minutes 45 seconds, 2.38 GB of ledger and
    about 12 GB of private job data existed but no PST part did. SIGTERM
    checkpointed and exited with status 130. The failed old and fresh jobs were
    removed after retaining bounded metrics, reclaiming about 14 GB.
  - [x] (2026-07-17) Replaced per-payload SQLite/file storage with one
    append-only durable pack with checked offset/length/hash slices, pack-first
    transaction durability, crash-tail truncation, one ordered event scan, and
    bounded failed-run cleanup. A 16 MB real-PST smoke produced 60 messages
    accepted by PSTForge, `pffinfo`, and `readpst`. Fast-gate evidence:
    `.agent/test-results/1784275380-fast`.
  - [x] (2026-07-17) Completed the automated 19 GB split in 9:35.13. One
    productive libpff pass durably found 37,373 candidates; a deterministic
    global recovery-index error was recorded as partial rather than rescanning
    the readable mailbox. Completed-store failures were bisected into valid
    groups. The retained result has 14 independently finalized PST parts,
    36,369 written candidates, 1,004 explicitly unsupported candidates,
    19,128,924,160 output bytes, and unchanged source identity. Private payload
    work was removed, leaving a 19 GB result job. External bounded evidence:
    `large-qualification-20260717T080346Z`.
  - [x] (2026-07-17) The owner accepted the measured 5,317,328,896-byte peak
    RSS as a known limitation for 0.4.1 after the 19 GB run completed within
    the operational time target. The 2 GiB objective remains an optimization
    and release-scale gate rather than blocking this split-validation
    milestone.
  - [x] (2026-07-17) Rejected the first completed 19 GB qualification at its
    first human gate. ScanPST found one invalid XBLOCK, 35,541 missing BBT
    references, two invalid folder table nodes, and re-added 4,241 messages as
    orphans. The first finding identified 8,064-byte aligned non-final row
    matrix blocks where the PST data tree requires the full 8,176-byte payload.
    The other findings are consequences of losing those table data trees.
    Testing stopped after part 0001; the other thirteen parts require no
    acceptance work. External evidence:
    `qualification-v041-pack-r5/part-0001.log`.
  - [x] (2026-07-17) Removed row-aligned data-tree chunking, taught the table
    reader to stream fixed-size rows across block boundaries, and added a
    large external-table regression requiring every non-final XBLOCK child to
    use the maximum payload. Fast gate:
    `.agent/test-results/1784293287-fast`.
  - [x] (2026-07-17) Repeated the automated 19 GB qualification with the
    corrected writer in 9:34.50. It again finalized 14 parts totaling
    19,128,924,160 bytes, wrote 36,369 candidates, explicitly marked 1,004
    unsupported, removed private payload work, and reverified source SHA-256
    `1552450f3ff27090aea4b20b461bb310c49d9999eaee89c65ca1a4b96e394ad5`.
    All 14 parts opened with Ubuntu `pffinfo` and completed a one-at-a-time
    `readpst` extraction; each extraction was deleted immediately and the
    private scratch directory is absent. Bounded evidence:
    `large-qualification-20260717T130341Z`.
  - [x] (2026-07-17) Rejected corrected job r6 at part 0001 ScanPST. The prior
    XBLOCK/BBT defect was eliminated: no invalid blocks, invalid nodes, or
    missing BBT references remain. ScanPST instead counted 306 of 310 Deleted
    Items rows and 3,877 of 3,931 Inbox rows, losing approximately one row at
    each external row-matrix block boundary; it then discarded both contents
    tables and orphaned all 4,241 messages. The repaired reference confirmed
    that rows remain block-aligned while unused tail bytes fill every non-final
    leaf to the required 8,176-byte payload.
  - [x] (2026-07-17) Implemented padded block-aligned row matrices, ignored
    arbitrary dead-space bytes when reading, and added BTH lookup assertions
    for the rows immediately before and after a padded boundary. A single
    clean-context review found and resolved the dead-space compatibility and
    boundary-lookup test gaps. Fast gate:
    `.agent/test-results/1784314227-fast`.
  - [x] (2026-07-17) Repeated the automated 19 GB qualification as r7 in
    10:15.36. It finalized 14 parts totaling 19,128,924,160 bytes, wrote 36,369
    candidates, explicitly marked 1,004 unsupported, and removed private
    payload work. All 14 parts passed Ubuntu `pffinfo` and complete
    one-at-a-time `readpst` extraction; extraction scratch was deleted
    immediately. Peak process RSS was 5,321,302,016 bytes, so the memory gate
    remains open. Bounded evidence: `large-qualification-20260717T185245Z`.
  - [x] (2026-07-17) The owner ran ScanPST on every original r7 part with no
    errors, opened every part in Outlook, and successfully imported the parts
    into MailPlus. This accepted the corrected row-matrix representation but
    exposed two product-level defects: part sizes varied widely below the
    requested target, and the visible hierarchy was wrapped in `Recovered
    Folder > Top of Outlook data file`.
  - [x] (2026-07-17) Replaced writer-error group subdivision with calibrated
    procedural packing of one deterministic ordered prefix. A validated trial
    that leaves room is extended; one that exceeds the maximum is reduced; no
    diagnostic half is published. Prefix bounds prevent oscillation, and
    exponential plus binary probing avoids whole-mailbox cloning per trial.
    Three focused reviews resolved retry and scale-complexity findings; the
    final focused review returned `CLEAN`.
  - [x] (2026-07-17) Corrected the adapted writer's FPMap start from allocation
    page three to page two. Examination of the owner's known-good 19 GB Outlook
    PST showed page zero AMap, page one PMap, page two FPMap, and data beginning
    at page three; the inherited offset overwrote data at the first FPMap
    boundary. A sparse 2,081,000,000-byte attachment regression crosses that
    boundary and passed completed-store validation.
  - [x] (2026-07-17) Removed the source store root and the source IPM subtree
    from canonical visible paths by source identity, not by localized display
    name or arbitrary depth. Well-known Deleted Items mapping now uses its
    source role/NID rather than name comparison, so an ordinary user folder
    named `Deleted items` remains distinct and retains its own mail.
  - [x] (2026-07-17) Rejected and automatically removed qualification r8 after
    its first two published parts exposed the obsolete raw-size estimator
    (2,058,806,272 and 2,074,551,296 bytes). Only 16 KiB of bounded evidence was
    retained at `large-qualification-20260717T202657Z`; no failed PST, payload
    pack, or extraction tree remains.
  - [x] (2026-07-17) Qualification r9 completed in 9:54.11 and finalized five
    PSTs. Parts 0001-0004 are 4,294,854,656; 4,293,837,824; 4,286,219,264; and
    4,274,791,424 bytes; part 0005 is the 1,977,541,632-byte final remainder.
    All five passed completed-store validation, Ubuntu `pffinfo`, and complete
    one-at-a-time `readpst` extraction before atomic publication. The run wrote
    36,369 of 37,373 candidates, explicitly accounted for 1,004 unsupported
    candidates, reverified the unchanged source SHA-256
    `1552450f3ff27090aea4b20b461bb310c49d9999eaee89c65ca1a4b96e394ad5`,
    and removed its 11 GB payload pack and reader scratch. Bounded evidence:
    `large-qualification-20260717T204931Z`.
  - [x] (2026-07-17) Updated the real-PST conformance fixture to compare
    source-visible paths after removing identified root/IPM infrastructure, and
    to exercise a 1 MiB part boundary now that artificial wrapper folders no
    longer inflate the output. Worker-fault expectations now match the
    milestone's one-failure unit isolation and no-rescan handling of a
    deterministic global parser error. The complete full gate passes formatting,
    warning-denied compilation and Clippy, all workspace and external-corpus
    tests, documentation and artifact checks, licensing, RustSec advisories,
    writer acceptance, and independent `pffinfo`/`readpst` checks. Evidence:
    `.agent/test-results/1784322570-full`.
  - [x] (2026-07-17) The final milestone-wide clean-context review found two
    high issues. The previously recorded 5.32 GB canonical-mail RSS remains an
    open milestone gate rather than a defect in the five published r9 parts.
    Resume replay identity now also includes the durable recovery unit, fixing
    the new correctness finding: a newly readable item with identical metadata
    in an earlier unit is retained even when the old candidate's worker ID
    shifts. A focused regression uses identical metadata across two normal
    units and passes. The post-remediation full gate passes at
    `.agent/test-results/1784323000-full`.
  - [x] (2026-07-17) The fresh replay remediation review found that an embedded
    candidate's parent worker ID can also shift. Replay identity now excludes
    only that transient `parent_message_id` while retaining the durable unit,
    embedded attachment path, and all stable start metadata. A nested regression
    shifts the parent ID and proves the committed embedded candidate is matched
    without discarding its newly readable parent. The full gate passes at
    `.agent/test-results/1784323233-full`.
  - [x] (2026-07-17) The next fresh review found the crash boundary between a
    committed parent and an uncommitted embedded child. A replay match now
    registers the exact durable item key and old source ID against the observed
    worker ID/path for that recovery unit. A new child resolves its parent
    through that exact mapping and stores the durable parent ID. The regression
    commits a parent, checkpoints, replays it under a shifted ID, adds its new
    child, and passes complete canonical ownership reconstruction. The full
    gate passes at `.agent/test-results/1784323598-full`.
  - [x] (2026-07-17) A fresh clean-context review of the replay ownership
    remediation returned `CLEAN`, with no blocker, high, or outcome-relevant
    medium resume/data-loss finding.
  - [x] (2026-07-17) The owner accepted r11 and closed the 0.4.1 human gate.
    ScanPST runs completed at that point were clean, Outlook attached an
    original part successfully, and its visible folders matched the source
    without the rejected recovery/store-root wrapper. The owner accepted this
    as proof of independently valid, tightly sized splits; remaining all-part
    content-fidelity analysis is not claimed by this milestone. Retain the
    50 GB PST for the later final scale gate.
  - [x] (2026-07-17) Rejected r9 after ScanPST of original part 0001; testing
    stopped before the other four parts. At an exact eight-AMap boundary,
    allocation rebuild wrote one inclusive extra PMap beyond the header EOF.
    ScanPST consequently saw a 1,024-byte physical/logical EOF mismatch,
    expected a nonexistent terminal AMap, and reported the extra PMap outside
    AMap coverage. Shared empty contents/hierarchy template blocks also retained
    one reference for the removed artificial wrapper, producing BBT/RBT counts
    `6 vs 5` and `7 vs 6`.
  - [x] (2026-07-17) PMap rebuild now uses ceiling page coverage without an
    inclusive endpoint, and shared template reference bases no longer count the
    removed wrapper. The ordinary writer suite passes 71 tests; the explicitly
    executed 2.081 GB sparse attachment regression crosses the first FPMap,
    verifies physical length equals header EOF, and passes completed-store
    validation in 120.45 seconds with automatic scratch cleanup. One full-gate
    run hit the unrelated post-publication signal timing assertion; its focused
    rerun passed without changes, and the repeated complete full gate passes at
    `.agent/test-results/1784324482-full`.
  - [x] (2026-07-17) A fresh focused review found that shared-table bases also
    had to depend on whether an explicit Deleted Items plan actually uses each
    default empty table. Contents and hierarchy reference counts now derive
    that condition separately. Regressions assert stored BBT counts for a
    nonempty explicit Deleted Items folder and for one with a child, including
    the ordinary user-created `Deleted items` sibling case. The full gate
    passes at `.agent/test-results/1784324705-full`.
  - [x] (2026-07-17) The remediation reviewer confirmed the separate contents
    and hierarchy predicates and found only the missing empty-Deleted-Items
    with nonempty-child test combination. That case now asserts that the shared
    contents reference is included while the shared hierarchy reference is
    excluded. The repeated full gate passes at
    `.agent/test-results/1784324986-full`.
  - [x] (2026-07-17) The final clean-context ScanPST remediation review returned
    `CLEAN`. Rejected r9 was removed after its 1,317-byte part-0001 log was
    retained externally; no r9 PST or private work remains.
  - [x] (2026-07-17) Qualification r10 completed in 10:38.05 and finalized five
    PSTs of 4,294,853,632; 4,293,837,824; 4,286,219,264; 4,274,791,424; and
    1,977,541,632 bytes. Every physical file length exactly matches its header
    EOF. Completed-store validation, `pffinfo`, and complete one-at-a-time
    `readpst` extraction passed before publication. The run wrote 36,369 of
    37,373 candidates, explicitly accounted for 1,004 unsupported candidates,
    preserved source SHA-256
    `1552450f3ff27090aea4b20b461bb310c49d9999eaee89c65ca1a4b96e394ad5`,
    and removed private payload and reader scratch. Peak RSS remains
    5,317,582,848 bytes. Bounded evidence:
    `large-qualification-20260717T215308Z`.
  - [x] (2026-07-17) Rejected r10 after ScanPST of original part 0001; no other
    part was tested. EOF, AMap, PMap, BBT, and NBT structure are now clean.
    Only shared BBT `cRef` arithmetic remained: raw BIDs `0x14` and `0x24`
    stored `4` and `5` while ScanPST's reference B-tree counted `5` and `6`.
  - [x] (2026-07-17) Corrected the shared-block formula to include the PST BBT
    ownership baseline in addition to NBT references, while retaining the
    separately reviewed Deleted Items contents/hierarchy predicates. Focused
    topology tests and the complete full gate pass at
    `.agent/test-results/1784326070-full`.
  - [x] (2026-07-17) Rejected r10 after ScanPST part 0001 showed the two shared
    counts had moved from one too high to one too low (`4 vs 5`, `5 vs 6`).
    This isolated the missing BBT ownership baseline; no new review was opened
    for the already-reviewed predicates. The 910-byte log was retained
    externally and all rejected r10 PST/private work was removed.
  - [x] (2026-07-17) Qualification r11 completed in 10:40.29 and finalized five
    PSTs of 4,294,853,632; 4,293,837,824; 4,286,219,264; 4,274,791,424; and
    1,977,541,632 bytes. Each physical length equals its header EOF, all
    completed-store and independent-reader checks passed before publication,
    source identity/SHA-256 remained unchanged, and private payload/reader
    scratch was removed. Peak RSS remains 5,317,328,896 bytes. Bounded
    evidence: `large-qualification-20260717T220903Z`.
  - [x] (2026-07-17) Recorded the owner's milestone boundary: 0.4.1 proves
    source-safe resume, practical 19 GB splitting, hard 4 GiB targeting,
    source-visible folder layout, and independently valid output PSTs.
    Attachment/content omission analysis is adjacent data-correctness work
    reserved for a separately planned 0.4.2 branch. No 0.4.2 implementation is
    included in this review unit.
  - [x] (2026-07-17) The final complete-diff review found one high-severity
    qualification-helper cleanup risk: any failed resumed run could recursively
    remove its pre-existing job. Cleanup is now armed only when the requested
    job path was absent for a fresh invocation; every resumed failure retains
    the job for diagnosis and another resume. A direct failed-resume regression
    preserved a sentinel, shell syntax passed, and the complete fast gate passes
    at `.agent/test-results/1784329593-fast`.
- [ ] Milestone 0.4.2: Incremental Data Correctness.
  - [x] (2026-07-17) The owner approved an incremental checkpoint workflow.
    Each data family produces one focused validation PST, pauses for ScanPST
    and Outlook, and receives a local and pushed branch commit only after human
    acceptance. Progress documentation travels with the corresponding code
    commit; it never creates a standalone commit.
  - [x] Checkpoint 1: private part manifests, bounded human `recovery.log`, and
    recursive attachments inside embedded items.
    - [x] (2026-07-17) Recursive translation and writer serialization now
      preserve attachments at every supported embedded-message level. The
      parser and writer now accept 256 embedded levels, based on measured stack
      behavior rather than an MS-PST format claim. Generated output is
      independently validated with `pffinfo` and `readpst`, and an unsupported
      embedded class is contained without discarding its readable parent
      message.
    - [x] (2026-07-17) New schema-6 jobs keep `parts/` PST-only and publish
      private part manifests beneath `.pstforge/manifests/` through the
      existing crash-reconciled intent transaction. A bounded mode-0600
      `recovery.log` is atomically regenerated for complete, partial, and
      interrupted split reports. This first checkpoint records exact aggregate
      omissions; source-folder grouping is installed with the durable omission
      ledger in checkpoint 2.
    - [x] (2026-07-17) Added the private command
      `cargo xtask qualify embedded-attachments <absolute-output>` and generated
      `qualification-v042-embedded-r3/parts/part-0001.pst` outside the
      repository. It contains a message attachment whose embedded message has
      both a by-value attachment and another embedded message with its own
      attachment and non-default attachment metadata. The 271,360-byte PST
      passed completed-store, `pffinfo`, and `readpst` validation with SHA-256
      `487e58c56b66cb9aa1bb63f60d39a6e4793f3d819b8b39fd63a975553699bdf8`.
      Superseded r2 was removed.
    - [x] (2026-07-17) The owner reported that r3 is clean in ScanPST and
      Outlook through `nested-payload.bin`. MailPlus was intentionally omitted
      because its known nested-message compatibility defect is tracked in an
      escalated Synology engineering ticket and does not indicate PST data
      loss. The owner approved commit, push, and continuation.
    - [x] (2026-07-17) The first checkpoint review found two highs and two
      outcome-relevant mediums. Repeated attachment-local node IDs could
      duplicate embedded message record keys; unbounded per-part log detail
      could fail after successful PST publication; interrupted reports could
      omit durable parts and unsupported counts; and completed-store
      validation checked only nested attachment row counts. The fixes derive
      embedded record keys from the full parent chain, cap detail at 10,000
      part lines while retaining exact totals, snapshot durable state for
      interrupted reports, and recursively reopen and compare nested
      attachment metadata/content. Rejected r1 was removed before r2.
    - [x] (2026-07-17) The fixed checkpoint passes the complete fast gate at
      `.agent/test-results/1784336515-fast`: formatting, locked workspace
      check, Clippy with warnings denied, all workspace tests, rustdoc,
      documentation/schema validation, and diff validation.
    - [x] (2026-07-17) A second focused review identified a real cleanup gap:
      the private qualification command relinquished its temporary-directory
      guard before atomic rename, so a rename failure could retain staging
      output. It now keeps the guard through rename. Two other findings were
      challenged rather than accepted: `create_fidelity_store` already invokes
      completed-store, `pffinfo`, and `readpst` validation before publication;
      and adding a translation-side depth cutoff would silently omit valid
      descendants based on an arbitrary number. The owner challenged the
      unmeasured 64-level parser/writer ceiling. A 256-level end-to-end fixture
      overflowed a 4 MiB test stack, passed at 6 MiB and 8 MiB, and passes with
      the writer's new explicit 32 MiB stack. The parser uses a pending-work
      stack rather than recursive calls. The supported ceiling is now 256 at
      intake and writing, with all 257 messages in the root-plus-descendant
      fixture translated, serialized, and recursively reopened.
    - [x] (2026-07-17) The final review also found that recursive validation
      compared nested filenames and payloads but not the nested attachment
      table and remaining property-context metadata. It now compares row size,
      number, method, rendering position, short and long filename, MIME type,
      content ID, content location, flags, object size, and payload identity at
      every level. The r3 fixture exercises non-default nested values.
    - [x] (2026-07-17) The r3 review found one remaining durable-state
      containment gap: a corrupt ledger could encode an embedded path beyond
      256 and reach recursive canonical reconstruction before writer
      validation. Canonical intake now rejects such ownership before building
      parent/child trees, with a direct durable-ledger corruption regression.
  - [ ] Checkpoint 2: lossless native intake for store, folder, associated,
    named-property, and arbitrary item-class data.
    - [x] (2026-07-17) Checkpoint 2a preserves numeric and string named-property
      identity from libpff through the supervised worker protocol, schema-7
      ledger, canonical reconstruction, and store-wide writer mapping. Durable
      identity records the GUID set and numeric identifier or string name;
      transient source `0x8000+` property IDs are never reused as identity.
      Malformed or unsupported named values remain explicit partial omissions.
    - [x] (2026-07-17) Added a one-purpose external corpus fixture containing
      one plain mail item, no attachments or recipients, and exactly two named
      properties: numeric PS_MAPI `0x8005` with a Unicode value and string
      `CustomCheckpoint` in a custom GUID set with an integer value. The
      ignored external-corpus regression independently reads source and output
      through libpff and compares GUID/name identity, type, byte length, and
      payload SHA-256. The native split completes with zero omissions and one
      271,360-byte candidate part at
      `qualification-v042-named-r1/parts/part-0001.pst`, SHA-256
      `c3c560f3f6c3e7ae4815c91dac04b7a117fda142ff88bf3cc764d69bcca25ed3`.
    - [x] (2026-07-17) Fresh adversarial review found that an interrupted
      schema-6 job could resume already-spooled properties without the newly
      durable name-to-ID identity and omit them. Checkpoint 2a bumps the job
      schema to 7 and explicitly refuses schema-6 resume; a regression mutates
      a fully bound ledger to the old version and proves both validation and
      resume refuse it.
    - [x] (2026-07-17) Final remediation review confirmed the schema refusal
      but found that the shared display-string decoder would replace invalid
      UTF-8 in a named-property string identity. Identity strings now use a
      strict decoder and malformed bytes produce a contained read issue instead
      of a different successful property name; display metadata remains lossy
      by design.
    - [x] (2026-07-17) The owner reported that
      `qualification-v042-named-r1/parts/part-0001.pst` passes ScanPST and
      Outlook, and approved the checkpoint for commit. Exact named-property
      identity and payload fidelity remain proven by the independent libpff
      source/output comparison because Outlook does not expose these properties
      in its ordinary message UI.
    - [x] (2026-07-17) Checkpoint 2b reconstructs every catalogued visible
      folder beneath the source IPM subtree independently of message placement
      and includes the complete visible hierarchy in part 0001. Empty parents
      and leaves are retained; system/search trees outside the IPM subtree
      remain excluded. Deleted Items is identified only by well-known source
      NID `0x8062`, so an ordinary user folder named `Deleted items` remains
      ordinary and distinct.
    - [x] (2026-07-17) Added the private `empty-folders` qualification source
      and a manifest-selected external-corpus regression. It compares every
      visible source/output path, including multiplicity, and Deleted Items
      role through independent libpff reads. The initial pre-review native
      split completed with zero omissions and produced one 271,360-byte
      candidate at
      `qualification-v042-empty-folders-r1/parts/part-0001.pst`, SHA-256
      `79cea37390706e25a8cbf0d28b56c9599be7c01e28253d0f2ff5003293173b22`;
      both source and output contain nine total folders and one message. This
      candidate is superseded by the schema-8 review remediations and must be
      regenerated before human acceptance.
    - [x] (2026-07-17) Checkpoint-2b adversarial review replaced source-NID
      ancestry with durable catalog-address ancestry, preserved duplicate
      native NIDs in tests, made duplicate visible paths an explicit counted
      folder omission, and changed the independent oracle from a set to a
      multiplicity-preserving sorted list. Job schema 8 refuses schema-7
      resumes because already-published part 0001 cannot be retrofitted with
      its empty hierarchy.
    - [x] (2026-07-17) A one-message part exceeding the hard maximum because of
      catalog-only folder overhead is no longer reported as an indivisible
      oversize message. PSTForge writes an exact message-only baseline; if that
      baseline fits, the enriched output is rejected as a conformance failure.
    - [x] (2026-07-17) A second focused adversarial review found two corrupt-ID
      collisions. IPM subtree recognition now requires both well-known NID
      `0x8022` and the direct-child catalog position, so a descendant reusing
      that NID remains visible without flattening. Message placement now
      normalizes a colliding display path to the retained folder role; writable
      mail is preserved while the unrepresentable duplicate folder remains a
      counted omission.
    - [x] (2026-07-17) Final focused review separated folder-set containment
      from message prefiltering. Each catalog-only folder is validated against
      a known-writable message and an invalid folder is counted and omitted
      without marking that message unsupported. Well-known Deleted Items
      selection is limited to one deterministic direct child of the actual IPM
      root; additional damaged folders reusing NID `0x8062` remain ordinary.
      A regression proves a source-only folder name beyond the writer's
      2,048-UTF-16-unit bound does not suppress valid mail.
    - [x] (2026-07-17) The focused remediation review is clean. Fast gate
      evidence is `.agent/test-results/1784341338-fast`; the independent
      external source/output folder comparison passes. The schema-8
      qualification candidate is
      `qualification-v042-empty-folders-r2/parts/part-0001.pst`, 271,360 bytes,
      SHA-256
      `79cea37390706e25a8cbf0d28b56c9599be7c01e28253d0f2ff5003293173b22`.
      Its bounded recovery log reports complete output and no skipped readable
      data; its part manifest reports zero folder, property, and attachment
      omissions; full verification reports nine reachable folders, one
      message, and no observed corruption. The source remains 271,360 bytes
      with SHA-256
      `a2739cdf474e3f94703a55173cf22605a230f637311806d20805332d909c3ef8`.
    - [x] (2026-07-17) Human acceptance passed for the checkpoint-2b schema-8
      r2 candidate: ScanPST reported clean output, Outlook opened the original
      unrepaired PST, the Inbox checkpoint message and every expected empty
      folder were present, the case-distinct ordinary `Deleted items` folder
      remained separate from well-known `Deleted Items`, and no artificial
      wrapper folder appeared.
    - [x] (2026-07-17) Pre-commit automation passed formatting, workspace
      checks, clippy, ordinary tests, documentation, artifacts, diff checks,
      licenses, advisories, writer acceptance, independent `pffinfo`, and
      independent `readpst`. The generic full gate could not enter its legacy
      corpus phase because the configured private manifest intentionally
      contains only the two focused 0.4.2 qualification cases and therefore
      has no `healthy_ansi` milestone-0.1.1 case. The applicable checkpoint-2b
      external libpff source/output comparison, full PSTForge verification,
      ScanPST, and Outlook acceptance all pass; the owner approved the
      checkpoint commit with this bounded corpus limitation.
    - [ ] Replace the native catalog's fixed 64-component `FolderAddress` with
      a resource-proportional durable address and measured deep-folder tests
      before claiming preservation beyond its current boundary. This is a
      worker/job compatibility change and is not hidden inside checkpoint 2b.
  - [x] Checkpoint 3: contacts.
    - [x] (2026-07-17) Native intake, canonical translation, and the writer now
      admit only `IPM.Contact` and descendants in addition to the previously
      supported mail/report classes. Contacts without mail sender fields no
      longer receive fabricated sender values or invalid empty Unicode
      streams. `PR_CONTAINER_CLASS` now travels explicitly from the native
      catalog through the worker protocol, schema-9 ledger, canonical folder,
      and writer input. The writer preserves `IPF.Contact` from the source
      instead of guessing from the current part's items; empty contact folders
      and later parts therefore retain their source class.
    - [x] (2026-07-17) Job schema 9 refuses schema-8 resumes because an older
      durable catalog may already have classified readable contacts as
      unsupported. The regression mutates a bound job to schema 8 and proves
      resume fails closed.
    - [x] (2026-07-17) Added external case `v042-contact-source`, 271,360 bytes,
      SHA-256
      `b4d6e250b96550d601721918c386dd3847283e248370716e59c132e56fb22ca7`.
      Its single contact contains display/given/surname, company, title,
      business and mobile phones, birthday, notes, File As, and Email1 fields.
      The ignored source/output regression compares message metadata, folder
      path and `IPF.Contact` class, ordinary property IDs/types/lengths/hashes,
      and PSETID_Address GUID/LID/types/lengths/hashes through independent
      libpff reads. The split reports one written contact and zero omissions.
    - [x] (2026-07-17) The initial complete fast gate passed at
      `.agent/test-results/1784343620-fast`. One pre-existing aggregate writer
      validation test exceeded Rust's default test-thread stack after the
      contact paths expanded validation; it now runs on the same measured
      32 MiB stack used by production writer validation.
    - [x] (2026-07-17) The first focused clean-context review found two
      checkpoint-relevant defects: content-based folder-class inference could
      misclassify mixed, empty, or later-part folders; and one-sided contact
      sender metadata could pass translation but fail completed validation.
      Source folder classes are now durable and later parts receive only the
      source folder definitions they use. One-sided contact sender pairs are
      cleared together and counted as one omitted property, keeping the
      contact writable and explicitly partial.
    - [x] (2026-07-17) Review remediation passes the focused external libpff
      source/output comparison and the complete fast gate at
      `.agent/test-results/1784344330-fast`. Focused regressions also prove an
      empty source `IPF.Contact` folder remains classified, later parts retain
      only source folder metadata they use, an ordinary item does not override
      its folder class, and one-sided contact sender metadata is contained
      without rejecting the part.
    - [x] (2026-07-17) The final focused review found one remaining medium
      later-part defect: exact leaf matching retained the leaf's source class
      but allowed the writer to synthesize nested ancestors as `IPF.Note`.
      Later parts now retain every source folder whose path is a prefix of a
      used message path. The regression uses `Contacts/Child` and proves both
      source-classified levels remain while an unused empty sibling is not
      replicated.
    - [x] (2026-07-17) The nested-ancestor regression, focused external
      contact roundtrip, and complete fast gate pass after remediation.
      Evidence: `.agent/test-results/1784344564-fast`.
    - [x] (2026-07-17) A fresh clean-context remediation review found no
      blocker, high, or medium findings after tracing source folder class
      preservation through nested later-part and resume behavior.
    - [x] (2026-07-17) Generated the single bounded human candidate at
      `qualification-v042-contact-r1/parts/part-0001.pst`: 271,360 bytes,
      SHA-256
      `21b35c20dc72b65e4b1b8bc2fe287d08f03dd988294c5347b96a874122e3f3e3`.
      PSTForge full verification reports Unicode64, no observed corruption,
      one complete item, zero unsupported items, and zero issues; `pffinfo`
      opens it successfully. The recovery log reports no readable data
      skipped, and transient schema-9 job/spool state was removed after
      verification.
    - [x] (2026-07-17) The owner reports all-green ScanPST and Outlook
      acceptance for the original, unrepaired contact candidate. This closes
      checkpoint 3 for commit and push.
  - [x] Checkpoint 4: appointments.
    - [x] (2026-07-17) Admit only `IPM.Appointment` and descendants as the next
      item family. Sender fields are optional and remain absent rather than
      receiving fabricated mail identities. Recovered appointments without a
      retained source folder default to `IPF.Appointment`; retained folder
      classes remain source-driven.
    - [x] (2026-07-17) The bounded non-recurring fixture follows the Microsoft
      MS-OXPROPS/MS-OXOCAL property contract: PSETID_Appointment contains busy
      status, location, UTC start/end, duration, all-day, state, and recurring
      fields; PSETID_Common contains reminder delta/time/enabled and common
      UTC start/end. Recurrence blobs and meeting semantics remain separate
      checkpoints.
    - [x] (2026-07-17) Job schema 10 refuses schema-9 resumes because an older
      durable catalog may already have classified readable appointments as
      unsupported. The focused regression proves the refusal.
    - [x] (2026-07-17) Added external case `v042-appointment-source`, 271,360
      bytes, SHA-256
      `dafa90a3840d1ac85bb0d9d72400114eaf7ec675094d1457beb1898e1fc4b20d`.
      Independent libpff intake confirms one senderless `IPM.Appointment` in
      an `IPF.Appointment` Calendar folder and exact named-property GUID, LID,
      type, length, and payload hashes.
    - [x] (2026-07-17) Measured source intake showed libpff exposes compact
      table-cell PT_BOOLEAN values as one byte, while property streams can use
      the two-byte MAPI form. The bounded scalar translator now accepts both
      encodings; the focused source/output regression passes with one complete
      item and zero folder, property, or attachment omissions.
    - [x] (2026-07-17) Complete fast automation passes at
      `.agent/test-results/1784346211-fast`; the ignored external appointment
      comparison passes again after the gate.
    - [x] (2026-07-17) The focused clean-context review found no blocker,
      high, or medium issue in the appointment class boundary, schema refusal,
      sender containment, bounded Boolean intake, exact named-property
      preservation, folder class, or writer validation.
    - [x] (2026-07-17) Generated the single bounded human candidate at
      `qualification-v042-appointment-r1/parts/part-0001.pst`: 271,360 bytes,
      SHA-256
      `85efb62e2b1430e38d1e4a9d2d0cae1c8a4ce384125fad4081a93ca8eba1ee64`.
      PSTForge full verification reports Unicode64, no observed corruption,
      one complete item, zero unsupported items, and zero issues; `pffinfo`
      opens it successfully. The recovery log reports no readable data
      skipped, and transient schema-10 job/spool state was removed after
      verification.
    - [x] (2026-07-17) The owner reports all-green ScanPST and Outlook
      acceptance for the original, unrepaired appointment candidate. This
      closes checkpoint 4 for commit and push.
  - [x] Checkpoint 5: meeting objects.
    - [x] (2026-07-17) Admit the exact `IPM.Schedule.Meeting.*` descendant
      family while rejecting the non-item root and missing-separator lookalike
      classes. Meeting objects retain ordinary mail sender/recipient rules.
    - [x] (2026-07-17) Job schema 11 refuses schema-10 resumes because an older
      durable catalog may already have classified meeting requests, responses,
      updates, or cancellations as unsupported.
    - [x] (2026-07-17) Added external case `v042-meeting-source`, 271,360
      bytes, SHA-256
      `702fddfff3876153813bc3759fa4157321f9226da3721834be860bf523a0544f`.
      It contains one initial non-recurring meeting request with organizer,
      To and Cc attendees, UTC start/end, location, state, reminder, meeting type,
      appointment message class, and valid 56-byte Global Object ID and Clean
      Global Object ID values. The independent libpff comparison requires
      exact message, routing, folder class, GUID/LID/type/length, and payload
      fidelity with zero omissions.
    - [x] (2026-07-17) The first split exposed ten false recipient-property
      omissions. All ten were deterministic recipient-table schema columns
      that the writer reconstructs, including row identity/version, role,
      display/address metadata, and optional empty columns. The mapped schema
      now matches the writer's complete recipient table; a focused regression
      and the external request roundtrip both pass with zero omissions. Both
      bounded debug jobs were removed.
    - [x] (2026-07-17) Complete fast automation passes at
      `.agent/test-results/1784347648-fast`; the exact ignored meeting
      roundtrip passes again after the gate.
    - [x] (2026-07-17) The focused review found one applicable high issue:
      recipient properties were considered reconstructed by tag alone, so a
      populated EntryID/SearchKey/RecordKey or a differing structural value
      could be changed while reporting zero omissions. Reconstruction now
      requires exact record set, type, and value equivalence for every source
      property. Populated optional binary values, legacy SMTP-address tags,
      type mismatches, or any differing role/identity/address value remain
      explicit omissions. The focused mismatch regression and exact meeting
      roundtrip pass; the final bounded debug job was removed.
    - [x] (2026-07-17) Recipient remediation passes the complete fast gate at
      `.agent/test-results/1784348066-fast`, the focused mismatch regression,
      and the exact ignored meeting roundtrip.
    - [x] (2026-07-17) The remediation review found one remaining medium
      accounting issue: libpff assigns each recipient's property record set
      that recipient's index, while the exact-equivalence check accepted only
      record set zero. The ownership check now matches
      `record_set_index == recipient.index`; the external fixture now has To
      and Cc rows so second-row schema, row identity/version, routing, and
      zero-omission behavior are exercised end to end.
    - [x] (2026-07-17) The two-recipient external roundtrip and complete fast
      gate pass at `.agent/test-results/1784348390-fast`; the superseded
      one-recipient external source was removed.
    - [x] (2026-07-17) The final focused clean-context review found no blocker,
      high, or medium issue after tracing exact recipient values and
      second-row ownership through the meeting roundtrip.
    - [x] (2026-07-17) Generated the single bounded human candidate at
      `qualification-v042-meeting-r1/parts/part-0001.pst`: 271,360 bytes,
      SHA-256
      `54beac2bb4d39c7ee9fbfb112e618f7f53add51912d35cd4e8e2fb345771570e`.
      PSTForge full verification reports Unicode64, no observed corruption,
      one complete meeting request, two recipients, zero unsupported items,
      and zero issues; `pffinfo` opens it successfully. The recovery log
      reports no readable data skipped, and transient schema-11 job/spool
      state was removed after verification.
    - [x] (2026-07-18) The owner reports all-green ScanPST and Outlook
      acceptance for the original, unrepaired meeting candidate. This closes
      checkpoint 5 for commit and push.
  - [x] Checkpoint 6: standalone tasks, sticky notes, and posts.
    - [x] (2026-07-18) Consolidated `IPM.Task`, `IPM.StickyNote`, and
      `IPM.Post` plus their dotted descendants because all three use the
      existing normal-message and lossless named-property path. Standalone
      tasks and sticky notes permit an absent sender and default to `IPF.Task`
      and `IPF.StickyNote`; posts retain normal sender rules and `IPF.Note`.
      `IPM.TaskRequest` is deliberately outside the admitted `IPM.Task`
      boundary because task communications have different sender, recipient,
      and embedded-task semantics.
    - [x] (2026-07-18) Job schema 12 refuses schema-11 resumes because an
      older durable catalog can already contain these three families marked
      unsupported.
    - [x] (2026-07-18) Added external case `v042-pim-source`, 271,360 bytes,
      SHA-256
      `1de2f8134c3e7fca9389977a909454a4cefdc0688ae9434ab1a802f99594d65f`.
      It contains one standalone task with task named properties, one sticky
      note with note display properties, and one post with sender identity in
      three exact source folders.
    - [x] (2026-07-18) The combined fixture exposed an existing completed-PST
      validator defect: its first-message validation built property IDs from
      that message's local named-property subset instead of the store-wide
      NAMEID map. Validation now uses the same store-wide identity ordering as
      serialization. The focused multi-message regression covers a
      lexically-first folder whose named-property GUID sorts after another
      message's GUID.
    - [x] (2026-07-18) Complete fast automation passes at
      `.agent/test-results/1784354764-fast`; the named-map regression and
      ignored exact libpff source/output roundtrip pass separately. The
      roundtrip writes three complete candidates in one part with zero folder,
      property, or attachment omissions and exact message, body, folder-class,
      named-property identity, type, length, and payload fidelity.
    - [x] (2026-07-18) The first focused review found one applicable medium
      evidence gap: named properties were compared as a store-wide aggregate,
      so the test did not prove that task properties remained on the task and
      note properties remained on the sticky note. Named-property
      fingerprints now carry their owning message class and subject. The
      fixture contract requires the task to own exactly six PSETID_Task
      properties, the sticky note to own exactly three PSETID_Note properties,
      and the post to own none.
    - [x] (2026-07-18) The owner-bound PIM roundtrip and the earlier named-
      property roundtrip pass, followed by the complete fast gate at
      `.agent/test-results/1784355041-fast`.
    - [x] (2026-07-18) The final fresh focused review confirms the prior
      owner-accounting gap is closed and reports no remaining blocker, high,
      or medium checkpoint-applicable finding.
    - [x] (2026-07-18) Generated the single bounded human candidate at
      `qualification-v042-pim-r1/parts/part-0001.pst`: 271,360 bytes, SHA-256
      `a6594d5bd8df45bf166baaa33f40c4448097744c56b37a34210cc6697f35f59c`.
      PSTForge full verification reports Unicode64, no observed corruption,
      three complete items, zero unsupported items, and zero issues;
      `pffinfo` opens it successfully. The recovery log reports no readable
      data skipped, the external source retains SHA-256
      `1de2f8134c3e7fca9389977a909454a4cefdc0688ae9434ab1a802f99594d65f`,
      and transient schema-12 job/spool state was removed.
    - [x] (2026-07-18) The owner reports all-green ScanPST and Outlook
      acceptance for the original, unrepaired PIM candidate. Outlook presents
      the task, sticky note, and post as their native item forms; the Post form
      shows the expected `Task notes checkpoint.` body. This closes checkpoint
      6 for commit and push.
  - [ ] Checkpoint 7: recurring-calendar exception objects.
    - [x] (2026-07-18) Read-only inspection of the completed 19 GB schema-5
      ledger found four remaining
      `IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}` items. All four
      are embedded children of two `IPM.Appointment` parents in Calendar and
      are owned by attachments carrying the MS-OXOCAL exception-link, original
      start/end, flags, hidden, display-name, encoding, and rendering-data
      properties. This evidence narrows the checkpoint to recurrence
      exceptions rather than generic OLE admission.
    - [x] (2026-07-18) Admit only the exact Microsoft calendar-exception class,
      case-insensitively, and preserve only attachment properties `0x3001`,
      `0x3702`, `0x3709`, and `0x7FFA..=0x7FFF` through a bounded raw-property
      path. Writer-managed attachment number `0x0E21` is explicitly
      reconstructed rather than falsely counted as omitted. Unknown OLE
      classes and exact-class suffix lookalikes remain unsupported.
    - [x] (2026-07-18) Job schema 13 refuses schema-12 resumes because the old
      durable catalog can already contain exact calendar-exception children
      marked unsupported. The focused refusal regression passes.
    - [x] (2026-07-18) Added external case
      `v042-calendar-exception-source`, 271,360 bytes, SHA-256
      `149b6e4eb4eddfe2a5dc26e48e4e1d91e26e6a41324120613b9d5c70a0e48e22`.
      It contains one Calendar appointment with one embedded exact-class
      exception object and all nine exception attachment properties.
      Independent libpff fingerprints bind property identifier, type, length,
      payload hash, attachment owner, exact embedded class, and embedded path.
    - [x] (2026-07-18) The focused CLI split writes two complete candidates in
      one part with zero folder, property, or attachment omissions. Exact
      source/output fingerprints match after excluding only `PR_ATTACH_SIZE`
      for embedded messages: that value is a derived byte size of the newly
      serialized child object, while binary attachment sizes remain exact.
    - [x] (2026-07-18) Complete fast automation passes at
      `.agent/test-results/1784378221-fast`; the exact ignored libpff
      source/output roundtrip passes again after the gate.
    - [x] (2026-07-18) The first focused review found one high containment
      defect and one medium boundary expansion. Malformed exception values
      could reject the whole appointment graph, and the exact OLE class/raw
      attachment fields were not structurally limited to an embedded child of
      an appointment.
    - [x] (2026-07-18) Remediation contains malformed exception values as
      property omissions. If required linkage cannot be retained, only the
      exception attachment and child become unsupported while the appointment
      remains writable and partial. Catalog intake rejects the exact exception
      class at top level, and writer validation requires an appointment parent,
      embedded exact-class child, and `0x7FFA..=0x7FFE` linkage. Focused
      containment, top-level, wrong-parent, missing-linkage, wrong-type, and
      exact external roundtrip tests pass.
    - [x] (2026-07-18) Remediation passes the complete fast gate at
      `.agent/test-results/1784378783-fast`; the exact ignored libpff
      source/output roundtrip passes again after the gate.
    - [x] (2026-07-18) The remediation review identified oversized-binary
      omission accounting and retained-evidence gaps, both applicable. It also
      raised duplicate raw IDs and catalog parent/linkage admission. Duplicate
      IDs were already contained by the shared attachment property-ID set
      before translation, now covered explicitly by the malformed exception
      regression. Catalog `supported` records class/intake capability; full
      parent and linkage structure is deliberately decided after the durable
      graph exists, where core marks the child unsupported before publication.
    - [x] (2026-07-18) An exception binary over the 1 MiB bounded-copy limit now
      increments exact omission accounting. The containment regression combines
      absent types, a duplicate linkage ID, and an oversized rendering payload:
      the parent remains writable, the unusable exception child and attachment
      are omitted, and all seven property omissions are counted. The external
      test now rechecks source identity after the split.
    - [x] (2026-07-18) Final remediation passes the complete fast gate at
      `.agent/test-results/1784379147-fast`. The exact external roundtrip and
      post-run source identity check pass with retained transcript at
      `.agent/test-results/1784379148-calendar-exception/external-roundtrip.log`.
    - [x] (2026-07-18) The final reviewer found one medium evidence-placement
      error: the post-run source identity assertion had landed in the earlier
      appointment test rather than this checkpoint's test. The assertion now
      executes in the calendar-exception roundtrip; its retained transcript is
      refreshed, and the complete fast gate passes at
      `.agent/test-results/1784379372-fast`.
    - [x] (2026-07-18) A fresh focused review confirms the source-identity
      assertion relocation and final uncommitted checkpoint state are clean
      with no blocker, high, or medium finding.
    - [x] (2026-07-18) Generated the single bounded human candidate at
      `qualification-v042-calendar-exception-r1/parts/part-0001.pst`: 271,360
      bytes, SHA-256
      `149b6e4eb4eddfe2a5dc26e48e4e1d91e26e6a41324120613b9d5c70a0e48e22`.
      PSTForge full verification reports Unicode64, no observed corruption,
      two complete items, one embedded message, one attachment, zero
      unsupported items, and zero issues; `pffinfo` opens it successfully.
      The recovery log reports no readable data skipped. Superseded source r1
      and bounded debug jobs were removed; source r2 remains the external
      manifest reference.
    - [x] (2026-07-18) The owner reports all-green ScanPST and Outlook
      acceptance for the original, unrepaired candidate. Outlook search finds
      the appointment and it opens and remains editable without corruption.
      The deliberately structural fixture has no parent appointment date, so
      Outlook displays no calendar date until one is assigned; exact hidden
      exception linkage is proven by the independent libpff roundtrip rather
      than visible recurrence UI. This closes checkpoint 7 for commit and
      push.
  - [x] Checkpoint 8: source-root hierarchy and associated/configuration data.
    - [x] (2026-07-18) Read-only inspection of the completed 19 GB schema-5
      ledger proves the remaining `IPM.Microsoft.SniffData` candidate is a
      complete normal message in `Freebusy Data`, whose parent is the PST
      store root rather than the IPM subtree. Its ten readable properties are
      ordinary MAPI values; no class-specific translation is required.
    - [x] (2026-07-18) The installed libpff 20231205 API exposes associated
      contents separately through
      `libpff_folder_get_number_of_sub_associated_contents` and
      `libpff_folder_get_sub_associated_content`. PSTForge currently calls
      neither function, so the old ledger cannot establish that the source has
      no additional hidden items.
    - [x] (2026-07-18) Extended recovery units, supervised events, durable
      state, canonical
      items, packing, and writer inputs with explicit folder location
      (`store_root` or `ipm_subtree`) and message placement (`normal` or
      `associated`). Placement comes only from source traversal, never a
      message-class or display-name heuristic. Worker protocol 3 carries the
      new field, and job schema 14 refuses schema-13 resume.
    - [x] (2026-07-18) Traverse every folder's normal and associated collections
      independently, with bounded counts, isolated per-item recovery units,
      source identity checks, crash containment, deterministic order, and
      durable resume compatibility.
    - [x] (2026-07-18) Reproduce store-root child folders under the output
      store root,
      retain IPM folders beneath the IPM subtree, emit associated messages as
      `AssociatedMessage` nodes in the owning folder's associated-contents
      table, and keep them out of normal contents and visible message counts.
      Associated-only parts are valid and independently reopen.
    - [x] (2026-07-18) Preserve arbitrary readable item classes by serializing
      their
      supported raw and named properties. A malformed or unsupported property
      makes only that item partial and is reported in `recovery.log`; class
      names alone must not discard an otherwise representable item.
    - [x] (2026-07-18) Focused writer, canonical, worker, and supervised CLI
      tests prove normal root placement and associated placement. The generated
      source and output fingerprints match folder parent location, placement,
      class, and custom raw-property identities, lengths, and hashes for both
      items, including an associated item's source `PR_DISPLAY_NAME` distinct
      from its subject fallback; `pffinfo` accepts the output and source
      identity is unchanged.
      The complete fast gate passes at
      `.agent/test-results/1784385982-fast`.
    - [x] (2026-07-18) Proved the final one-part qualification candidate with
      writer/parser tests and a one-part external libpff roundtrip. The source
      and output fingerprints include folder parent location, placement,
      class, and every representable property; independent `pffinfo` opened
      the candidate before ScanPST and Outlook acceptance.
      Candidate:
      `qualification-v042-associated-r1/parts/part-0001.pst`,
      271,360 bytes, SHA-256
      `4c65af2b3ed1f51ec3f9be459eb3f4fdf7525f9839ea24521b3c69f1be333256`.
      PSTForge verification found zero corruption, traversal issues, or
      unsupported messages, `pffinfo` 20231205 opened it as a Unicode 64-bit
      PST, and `recovery.log` reports that no readable data was skipped.
    - [x] (2026-07-18) ScanPST rejected the r1 candidate because associated
      message node `0x200048` lacked `MSGFLAG_ASSOCIATED`; its associated
      contents-table row therefore did not match the sub-object. The repaired
      reference retained both items and provided a comparison baseline.
      Microsoft explicitly requires `MSGFLAG_ASSOCIATED` in
      `PidTagMessageFlags` for every message in an associated contents table.
      Writer flag generation now derives that bit from typed placement: it is
      forced in both the message property context and associated table for
      associated messages, and cleared for normal and embedded messages even
      if untrusted source flags contain it. Completed-store validation and the
      supervised libpff roundtrip assert the same invariant.
    - [x] (2026-07-18) Generated and independently verified the r2 candidate:
      `qualification-v042-associated-r2/parts/part-0001.pst`,
      271,360 bytes, SHA-256
      `6e179d8980fc23b4eba5cd776a4b169f93b24a59df22d1f30039c05a994e7cdf`.
      PSTForge verification found zero corruption, traversal issues, or
      unsupported messages; the placement fingerprint includes
      `PidTagMessageFlags`; `pffinfo` 20231205 opened the Unicode 64-bit PST;
      and `recovery.log` reports that no readable data was skipped.
    - [x] (2026-07-18) The owner reports that the unrepaired r2 candidate
      passes ScanPST and opens cleanly in Outlook. Associated configuration
      objects are intentionally not visible as ordinary messages; clean
      ScanPST and stable Outlook attachment close the human acceptance gate.
  - [ ] Checkpoint 9: writer-wide Microsoft specification conformance audit.
    - [x] (2026-07-18) Created `docs/WRITER_CONFORMANCE.md` as the
      non-destructive audit ledger, with source baselines, status semantics,
      subsystem coverage, code/test/evidence mappings, and an explicit exit
      gate. The initial inventory isolates the fixed HMP hierarchy map, the
      persisted-view/index templates, and the empty NAMEID sentinel as
      empirical or partially documented output. All remain unchanged pending
      exact source comparison and, where documentation remains absent, human
      disposition.
    - [x] (2026-07-18) Completed the first exact Messaging-layer pass over
      mandatory nodes, fixed folders, six required table templates, store,
      folder, message, recipient, and attachment minimum schemas. The core
      required graph and ordinary object schemas match. The audit found
      Microsoft-source contradictions for replication/container-class template
      types and fixed-folder content counts, plus undocumented ScanPST-derived
      output (`0xEC1`, store property `0x6633`, provider/index table schemas,
      and replication-instance values). The conformance index preserves and
      isolates all of them for control comparison and human disposition; no
      writer bytes changed.
    - [x] (2026-07-18) Completed the exact Unicode HEADER and ROOT comparison.
      The documented field layout, constants, reserved bytes, free-map fields,
      root references, counters, and CRC ranges match MS-PST 2.2.2.5-2.2.2.6.
      The Outlook-control-derived `bidUnused` and `dwUnique` creation seed is
      not prescribed by Microsoft, so it is now isolated as EMP-10 and retained
      unchanged for human disposition. No writer bytes changed.
    - [x] (2026-07-18) Completed the exact NDB page, block, data-tree,
      subnode-tree, BBT/NBT, and allocation-map comparison. Physical
      alignment, trailers, checksums, signatures, block types, ordinary tree
      levels, B-tree capacities, and DList-based allocation match. Two bounded
      implementation gaps are isolated for focused correction: a zero-byte
      spooled value currently reaches a rejected zero-length data block, and a
      local subnode set above the single documented SIBLOCK level (roughly
      173,000 entries) would attempt an invalid level 2 rather than refusing
      cleanly. No writer bytes changed.
    - [x] (2026-07-18) Completed the exact LTP heap, BTH, PC, TC, column,
      row-matrix, and existence-bitmap comparison. The structural encodings
      match MS-PST. Two already accepted value encodings are not fully
      reconciled with generic MS-OXCDATA wording: PSTForge omits the Unicode
      terminator because terminated controls caused visible MailPlus suffix
      corruption, and fixed-width multivalues use packed elements with count
      inferred from allocation length. Both are isolated as EMP-11/EMP-12 and
      retained unchanged pending control comparison and human disposition. No
      writer bytes changed.
    - [x] (2026-07-18) Completed the populated NAMEID map comparison. Store-wide
      identity collection, property-index assignment, reserved/custom GUID
      selectors, entry/string/GUID streams, 251-bucket hashing, and embedded
      message lookup match MS-PST 2.4.7. The fixed empty-map mapping and the
      physical MAPI GUID used when no custom GUID exists remain EMP-03 because
      Microsoft does not require those sentinels; both remain unchanged. No
      writer bytes changed.
    - [x] (2026-07-18) Verified embedded-message serialization against MS-PST
      2.3.3.5, 2.4.5, and 2.4.6.3. Method 5, the attachment-owned object
      subnode, NID/size pair, child property context, recipient and attachment
      tables, and recursive child subnodes agree. The accepted 256-level
      traversal bound is a product containment policy rather than a PST
      boundary. No writer bytes changed.
    - [x] (2026-07-18) Verified body serialization against MS-OXCMSG and
      MS-OXRTFCP. Plain text, binary HTML, compressed RTF, RTF synchronization,
      native-body enumeration, and Internet code page use the documented
      property IDs and types; the literal-only LZFu stream has the required
      header, termination reference, and CRC. PSTForge does not fabricate an
      absent body representation. No writer bytes changed.
    - [x] (2026-07-18) The exact MS-OXOCAL comparison found a conflict in the
      already accepted calendar-exception fixture. Microsoft defines exception
      replacement time as `0x7FF9/PtypTime`; the source and current exact
      output instead contain `0x7FFA/PtypInteger32`, also retain
      `0x7FFF/PtypBoolean` (canonically the attachment-contact-photo flag), and
      require `0x7FFA..=0x7FFE` for admission. The documented start, end,
      flags, hidden state, method, embedded class, and object relationship
      agree. The conflicting source fields are isolated as EMP-13 and remain
      byte-exact; no property was stripped, remapped, synthesized, or changed
      pending owner disposition. No writer bytes changed.
    - [x] (2026-07-18) Split generic property fidelity from class semantics.
      MS-OXOMSG, MS-OXOCNTC, MS-OXOCAL, MS-OXOTASK, MS-OXONOTE, MS-OXOPOST,
      and MS-OXOCFG now index the completed mail, contact, calendar/meeting,
      standalone task, sticky-note, post, and associated-configuration
      checkpoints. Exact source/output fingerprints prove that supported raw
      and named values remain on the correct source class and owner. Later
      distribution-list, task-communication, document, and journal families
      remain pending their own protocol pass.
    - [x] (2026-07-18) The generated-metadata audit found mixed fallback
      behavior when source values are absent: `(no subject)`, `Unknown Sender`,
      copied sender halves, `MSGFLAG_READ`, UTF-8 Internet codepage,
      received-time creation/modification substitutes, standard folder/message
      class fallbacks, and subject fallback for absent FAI display names.
      Structural PST requirements, standard container classes, and the existing
      product codepage contract justify part of that set, but no located
      Microsoft source requires the user-visible subject/sender substitutions
      or content-derived classification when source class is absent. EMP-14
      retains all current behavior until the owner decides how structurally
      required fields, provenance, and partial accounting should replace or
      preserve those values. No writer bytes changed.
    - [x] (2026-07-18) Recipient and attachment fallbacks are now isolated as
      EMP-15. The pipeline can copy a missing recipient display name/address
      from its counterpart; generate recovery filenames; label embedded
      messages `message/rfc822`; and default rendering position/flags. Required
      table shape and `-1` rendering semantics are documented, while generated
      display/MIME values are recovery policy. All remain unchanged pending
      owner disposition; usable payloads must not be stripped merely because
      optional display metadata is absent.
    - [x] (2026-07-18) Closed the publication/integrity pass. PSTForge builds a
      private new file rather than modifying a published PST, synchronizes it,
      completes and resynchronizes any allocation-map rebuild, validates its
      owned object graph, requires independent `pffinfo` and `readpst`
      acceptance, atomically renames without replacement, synchronizes the
      held destination directory, and verifies the published device/inode.
      Existing failure, timeout, moved-directory, no-clobber, retained-evidence,
      and scratch-containment tests cover the publication states. No writer
      bytes changed.
    - [x] (2026-07-18) Resolved the two normative NDB gaps without removing or
      changing empirical output. Empty binary/Unicode values continue through
      the inline empty-value representation; zero-byte spool descriptors are
      now rejected during writer preflight and cannot emit a prohibited
      zero-length data block. Subnode trees now enforce the exact 340-leaf by
      510-intermediate capacity before appending blocks, so a 173,401st local
      subnode cannot create the MS-PST-prohibited second SIBLOCK level. Both
      focused regressions pass.
    - [x] (2026-07-18) The complete fast gate passes at
      `.agent/test-results/1784387912-fast`. A fresh clean-context adversarial
      review scoped to valid-input preservation, integer bounds,
      mutation-before-error, and PST validity reports CLEAN with no blocker,
      high, or medium finding. The zero-spool guard changes no valid output
      bytes, and the subnode limit is unreachable under the writer's existing
      per-message collection bounds; no ScanPST candidate is needed for these
      preflight-only corrections.
    - [x] (2026-07-18) The initial pre-commit full gate passed format, check, clippy,
      workspace tests, documentation, artifact checks, licenses, advisories,
      independent `pffinfo`, independent `readpst`, and writer acceptance. The
      first run stopped because `PSTFORGE_CORPUS_MANIFEST` was unset. Re-running
      with the ExecPlan-documented external manifest at
      `$XDG_DATA_HOME/pstforge-test-corpus/manifest.toml` reached the corpus
      phase and stopped because that focused manifest has no `healthy_ansi`
      case classified `milestone_0_1_1`. Evidence is retained at
      `.agent/test-results/1784388172-full`. Do not commit until the required
      external case is available or the owner explicitly approves a recorded
      gate exception. The combined manifest below resolved the blocker; no
      exception was taken.
    - [x] (2026-07-18) Replaced the incomplete focused/full manifest choice
      with an external mode-0600 combined manifest containing the legacy
      ANSI/Unicode/split cases and every focused 0.4.2 case. The combined run
      passed all six prior 0.4.2 source/output comparisons and the calendar
      exception comparison. Its legacy GroupDocs split case then exposed one
      known bounded `libpff_message_get_recipients` failure in addition to the
      already permitted attachment-count failures. The source verification
      reports that message incomplete, and the split regression already
      requires incomplete source messages to produce partial accounting. The
      independent fingerprint helper now permits only those two exact libpff
      operations/messages and continues to reject every other issue, dropped
      issue, or unfinished stream.
    - [x] (2026-07-18) The legacy full-corpus split then exposed two stale test
      assumptions and one applicable reporting defect. Private part sidecars
      moved from `parts/` to `.pstforge/manifests/` in checkpoint 1, and parts
      now preserve empty source folders rather than equating folder count with
      message-bearing leaves. The regression now validates the private path,
      requires all message paths to be represented, and accounts for mandatory
      writer folders separately. More importantly, part `message_count`
      included normal and recursively embedded messages but omitted associated
      messages. Counting both top-level collections corrects the private
      manifest and human/JSON report without changing PST bytes.
    - [x] (2026-07-18) Accepted exact-length UTF-16LE property and table
      allocations as the permanent EMP-11 interoperability exception. The
      focused writer roundtrip freezes the absence of a trailing NUL; the
      earlier strict-NUL output produced visible folder/subject corruption,
      while the retained form passed ScanPST, Outlook, and MailPlus. No writer
      bytes changed in this checkpoint.
    - [x] (2026-07-18) Implemented permanent typed reconstruction accounting
      for EMP-14 and EMP-15 without changing any fallback value. Each part
      records bounded grouped counts for metadata derived from other readable
      source values and metadata generated because the source field was absent
      or unusable.
      `recovery.log` renders the aggregate without subjects, addresses, paths,
      attachment names, or item identifiers. Associated and recursively
      embedded messages contribute to the aggregate. Reconstruction remains
      separate from `partial`, which still means readable source data was not
      preserved. Private sidecar schema 1.1.0 and job schema 15 prevent an old
      resume from silently losing the new accounting.
    - [x] (2026-07-18) Completed the consolidated no-PST-byte recovery point.
      The combined-manifest full gate passed formatting, check, Clippy,
      workspace tests, documentation/artifact policy, licenses, advisories,
      writer acceptance, every focused 0.4.2 comparison, the legacy damaged
      split control, and independent `pffinfo`/`readpst` checks. Evidence:
      `.agent/test-results/1784392888-full`. One fresh clean-context
      adversarial review focused on accounting completeness, privacy, resume
      compatibility, partial status, source-loss masking, the NDB guards, and
      the unchanged valid-PST serialization boundary returned `CLEAN`. Human
      approval remains required before the focused 0.4.2 commit and push.
    - [x] (2026-07-18) Resolved EMP-12 as a documentation/test recovery point.
      MS-PST 2.3.3.4.1 explicitly requires fixed-width multivalue PST
      allocations to contain a tightly packed array with count derived from
      allocation length. The current reader/writer already implements that
      representation. Correct inherited generic wire-format comments, freeze
      every supported fixed-width type with focused encoding tests, and
      classify LTP-03/EMP-12 as verified. The combined-manifest full gate
      passed at `.agent/test-results/1784393719-full`. An initial focused
      review found that the GUID test did not prove all encoded fields; the
      complete 32-byte representation and every decoded field are now asserted,
      and a fresh final-state review returned `CLEAN`. Production code and PST
      bytes are unchanged, so no human ScanPST candidate is required. Human
      approval remains required before commit and push.
    - [x] Create `docs/WRITER_CONFORMANCE.md` with one traceable row for every
      existing store, NDB, LTP, folder, message, recipient, attachment,
      embedded-message, associated-content, named-property, and publication
      invariant. Each row names the authoritative Microsoft document,
      revision/page date, exact section or property, implementation symbol,
      focused test, and independent evidence.
    - [ ] Audit the full existing writer against that index before admitting
      another item class. Treat an undocumented literal or relationship as an
      unresolved requirement, not as established behavior. Preserve
      undocumented existing output until the owner decides whether to retain,
      revise, or remove it.
    - [ ] Resolve verified gaps as separate reviewed recovery points. Each fix
      must add the normative reference and regression before code changes are
      accepted, and each structural output change requires ScanPST-first human
      acceptance.
    - [ ] Require every remaining 0.4.2 checkpoint to complete its Microsoft
      specification entries before implementation begins.
  - [ ] Checkpoints 10 onward: distribution lists; documents and reference
    attachments; task communications; journal/activity objects; and remaining
    generic classes, each proven separately where feasible.
  - [ ] Run the complete 19 GB split once after all focused checkpoints pass
    and reconcile discovered unique items against written plus explicitly
    unwritten items across every part.
- [ ] Milestone 0.5.0: Operational UX and Debian Packaging.
- [ ] Milestone 0.5.1: GitHub CI and Private-Corpus Automation; the remote is
  reachable, and work begins after the approved baseline is pushed.
- [ ] Milestone 0.6.0: Interoperability Release Candidate.
- [ ] Milestone 1.0.0: MailPlus-Ready Release.

## Surprises & Discoveries

- Observation: The original durable-spool design performed one file publish,
  file sync, directory sync, and later rehash per property. A 19 GB corrupt
  source therefore amplified small properties into tens of thousands of files,
  terabytes of logical reads, and no visible output because PST publication is
  intentionally a second phase. SQLite candidate events also lacked an index
  on `blob_sha256`, making orphan-blob validation quadratic at realistic job
  scale.

- Observation: Durable replay cannot use libpff message IDs or ledger row
  position as its sole identity. Synthetic embedded IDs shift when earlier
  damaged units are skipped, and a failed unit creates a legitimate gap before
  later durable candidates. A hashed multiset of immutable provenance,
  recovery index, and metadata lets new gaps proceed while requiring every
  durable candidate to be observed before completion.

- Observation: The successful 19 GB run met the 20-minute target and published
  its first part in under six minutes, but whole-mailbox canonical event
  materialization peaked at 5.32 GB RSS. The payload pack solved disk and I/O
  amplification; canonical reconstruction must still become a bounded
  candidate/part stream to satisfy the 2 GiB gate.
  Evidence: read-only `/proc` and ledger inspection of the interrupted owner
  run plus the 2026-07-17 real-PST before/after benchmark.

- Observation: A table row matrix is a logical byte stream; its row boundaries
  may cross data-block boundaries. Aligning each leaf block to the row width
  produced 8,064-byte non-final blocks instead of the required 8,176 bytes.
  PSTForge's former block-by-block table reader concealed the defect, while
  ScanPST rejected the XBLOCK and consequently orphaned 4,241 folder messages.
  Evidence: `qualification-v041-pack-r5/part-0001.log` and focused writer
  regressions in `.agent/test-results/1784293287-fast`.

- Observation: External table row-matrix leaves have two simultaneous
  constraints: rows cannot cross leaf boundaries, and every non-final XBLOCK
  child must consume the full 8,176-byte payload. The unused tail after the
  final complete row is dead space whose contents readers must ignore. The r6
  writer satisfied only the second constraint, causing ScanPST to lose one row
  at nearly every boundary.
  Evidence: r6 part-0001 ScanPST log and repaired comparison, plus
  `.agent/test-results/1784314227-fast`.

- Observation: Moving small payloads into SQLite removed file-count growth but
  did not remove work amplification. On the fresh 19 GB run, recovery produced
  714,994 inline payloads and 3,616,287 event rows. Canonical reconstruction
  then issued tens of thousands of separately prepared event queries and
  reread roughly 29 GB logically every 15-20 seconds without publishing a
  part. The mailbox had already been parsed successfully; normalizing and
  replaying the entire mailbox before output was redundant.
  Evidence: `/proc` I/O deltas, read-only ledger counts, bounded run log, and
  `/usr/bin/time -v` from the interrupted 14:45 qualification run.

- Observation: The owner will use a 19 GB corrupt PST for the first large-file
  qualification because moving the 50 GB source and derived data is
  unnecessarily expensive. The 50 GB source remains the final 1.0 scale gate.
  Evidence: owner direction on 2026-07-16.

- Observation: A Linux supervisor killed with SIGKILL cannot clean up its
  native parser child. The worker therefore arms `PR_SET_PDEATHSIG` through
  rustix and verifies the expected parent after arming it; the external gate
  confirms the worker exits and the durable job resumes.
  Evidence: focused forced-kill external-corpus run on 2026-07-16.

- Observation: An embedded-message attachment requires two distinct local
  descriptor levels: the parent message references the attachment PC, and the
  attachment PC's own descriptor tree references the embedded message PC and
  its recipient/attachment tables. Placing the embedded message directly in
  the parent descriptor tree lets simple readers see the attachment row but
  makes `libpff` reject the object lookup.
  Evidence: the generated 0.2.1 fixture initially reported one attachment and
  an invalid local-descriptor lookup; after the nested descriptor correction,
  `libpff` reported two attachments and one embedded message without issues.

- Observation: The completed 19 GB ledger contains four unsupported OLE-class
  items, but all four use Microsoft's exact calendar-exception class and are
  embedded under two recurring appointments. Their owning attachments contain
  the documented MS-OXOCAL `0x7FFA..=0x7FFF` exception linkage and timing
  fields. The only other unsupported item in that source is one root-level
  `IPM.Microsoft.SniffData` configuration item, so generic OLE admission would
  be broader than the measured source requires.
  Evidence: read-only SQLite inspection of
  `/storage/PSTForge/qualification-v041-pack-r11/.pstforge/job.sqlite3` and
  Microsoft MS-OXOCAL property contracts on 2026-07-18.

- Observation: The remaining `IPM.Microsoft.SniffData` item is not hidden
  associated content. The schema-5 ledger records it as a normal, complete
  candidate in source folder node 33474 (`Freebusy Data`), directly below
  source root node 290, with ten readable ordinary properties. Separately,
  libpff exposes associated contents through folder APIs that the current
  catalog does not call, so associated-data completeness cannot be inferred
  from the old ledger.
  Evidence: read-only SQLite inspection of the completed 19 GB ledger and
  `/usr/include/libpff.h` from libpff 20231205 on 2026-07-18.

- Observation: Associated/configuration items may carry `PR_DISPLAY_NAME`
  independently of `PR_SUBJECT`. Using the normalized subject fallback in the
  associated-contents table overwrites that source identity even when the
  message property context retains it.
  Evidence: checkpoint-8 adversarial review and the subject/display-name
  divergence regression through the supervised libpff roundtrip on 2026-07-18.

- Observation: A node of type `AssociatedMessage` and membership in an
  associated contents table are not sufficient for Outlook consistency.
  `PidTagMessageFlags` in the message property context and table row must also
  carry `MSGFLAG_ASSOCIATED` (`0x00000040`). ScanPST reports a row/sub-object
  mismatch when that bit is absent.
  Evidence:
  `qualification-v042-associated-r1/parts/part-0001.log`, its repaired PST,
  and Microsoft's Folder-Associated Information Tables contract on 2026-07-18.

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

- Observation: The older recovery path recorded only a named property's
  store-local `0x8000+` identifier even though libpff's lower-level record-entry
  API also exposes its name-to-ID map entry. Reusing the transient identifier
  in another PST would silently change meaning.
  Evidence: checkpoint 2a now calls
  `libpff_record_entry_get_name_to_id_map_entry`, captures GUID plus numeric or
  string name before the property stream, transports it through the worker and
  durable ledger, and assigns a new output-local ID from that identity.

- Observation: Deleting a publication scratch directory by a pathname derived
  from a held descriptor creates a same-UID stale-path race after another
  process renames and replaces that pathname.
  Evidence: clean-context review demonstrated the replacement window. Version
  0.4.0 leaves only empty private `.pstforge-*` directories after successful
  publication; bounded stale-directory cleanup belongs to 0.4.1 resume/cleanup
  semantics.

- Observation: A sidecar leaf-folder count cannot validate total generated
  folder inventory because a part may replicate additional source hierarchy
  prefixes. The GroupDocs first part has two leaf folders, three replicated
  source hierarchy nodes, and six mandatory writer folders.
  Evidence: independent libpff traversal reports nine folders. The external
  gate now derives leaf and prefix sets separately rather than relying on the
  coincidentally correct `leaf count + 7` formula.

- Observation: `compressed-rtf` 1.0.1 verifies the container CRC but treats EOF
  or a truncated token run as successful decompression, so decoded length alone
  does not prove the required LZFu end reference was present.
  Evidence: a correctly rechecksummed fixture with its final end reference
  removed was accepted by the dependency and rejected by PSTForge's bounded
  structural token walk. Uncompressed containers also require the specified
  zero CRC before preservation.

- Observation: `libpff_message_get_number_of_attachments` can return a native
  error for a message with no readable attachment table; the same error family
  can also represent a damaged or unreadable table and therefore cannot be
  safely reclassified as zero attachments from its backtrace or message flags.
  Evidence: public and generated healthy fixtures produced descriptor-1649
  count errors, while upstream semantics show those strings wrap descriptor
  lookup/read failures. Version 0.4.0 records count uncertainty as candidate
  partial, and the external gate compares recoverable content while requiring
  that incomplete source messages remain visibly partial.

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

- Observation: The first four 0.4.0 split candidates passed PSTForge's
  independent readers but exposed progressively narrower defects in Microsoft
  ScanPST: invalid data-tree totals and references, oversized or incompletely
  described heap allocations, missing copied contents-table properties, and
  incorrect unread aggregates. Candidate r5 resolves those findings. The owner
  reports a clean ScanPST run and successful Outlook opening of both parts with
  the expected folder and unread counts. The ScanPST log itself was not copied
  into the external acceptance directory, so the human report is the retained
  acceptance evidence.
  Evidence: focused writer/core regressions, r1-r4 external ScanPST logs and
  repaired comparisons, and owner acceptance of r5 on 2026-07-16.

- Observation: MailPlus imports the nine Inbox messages from the two accepted
  parts into one Inbox but does not import the message stored under Deleted
  Items. Outlook and the PST itself expose that tenth message in Deleted Items,
  so the difference is an importer policy rather than loss during recovery,
  packing, or PST serialization.
  Evidence: owner comparison of the same r5 artifacts in Outlook and MailPlus
  on 2026-07-16.

- Observation: A message NID is a stable global deduplication key only for a
  top-level NBT message. Embedded-message NIDs belong to separate attachment
  subnode trees and can repeat without identifying the same message. The
  writer's repeated local embedded NID values were exposed by libpff as distinct
  identifiers in a generated experiment, so that experiment was removed rather
  than retained as a false regression; the scope rule is tested directly.
  Evidence: MS-PST subnode ownership, the durable parent/path model, focused
  libpff-sys tests, and final clean-context review on 2026-07-16.

- Observation: The adapted upstream writer placed the first FPMap one allocation
  page too late. At the first large-file FPMap boundary this caused allocation
  metadata to overwrite a data page, while smaller generated PSTs remained
  unaffected and therefore could not expose the defect.
  Evidence: the owner's known-good 19 GB Outlook PST places AMap, PMap, FPMap,
  and the first data page at consecutive allocation pages zero through three;
  the corrected sparse 2,081,000,000-byte attachment regression crosses the
  boundary and passes completed-store validation.

- Observation: Raw recovered-property bytes are not a stable predictor of
  serialized PST size. Table overhead, allocation maps, compression, and
  message shape made the old estimate underfill ordinary r7 parts by roughly
  half. Measuring a validated procedural trial and calibrating the same ordered
  prefix produced four non-final r9 parts within 20,175,872 bytes of 4 GiB.
  Evidence: r8 bounded evidence
  `large-qualification-20260717T202657Z` and successful r9 evidence
  `large-qualification-20260717T204931Z`.

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

- Decision: Version 0.3.1 aggressive mode passes both documented libpff flags,
  but does not relabel the generic recovered-item collection as fragment data.
  `fragment` provenance is emitted only when the native boundary can prove
  fragment origin. The supported upstream libpff accepts
  `SCAN_FOR_FRAGMENTS`, but its raw data-block path ends at an unimplemented
  `consider data block as fragment` marker and exposes no per-item origin API.
  Rationale: allocation-ignore results include ordinary recovered index
  entries. Calling all of them fragments would create false provenance and
  could distort later packing and recovery claims. A future fragment
  constructor/provenance API belongs in the separately licensed LGPL fork and
  must retain distinct lower-confidence accounting.
  Date/Author: 2026-07-16 / Codex after upstream source review.

- Decision: Fault-isolation addresses use a maximum-64-level folder
  child-index path, not folder identifiers or traversal ordinals. Folder
  metadata/child enumeration and top-level messages are announced as separate
  units. Every committed candidate stores its unit in the private ledger;
  replay omits candidates in isolated units while final accounting retains
  them.
  Rationale: folder identifiers can be zero or duplicated, traversal ordinals
  shift when a damaged subtree is skipped, and a message unit can commit an
  outer message before a later embedded child crashes. Stable paths plus
  unit-bound durable candidates let the supervisor isolate the exact subtree
  or top-level message without discarding prior progress or cascading replay
  mismatches into unrelated mail.
  Date/Author: 2026-07-16 / Codex after adversarial review.

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
  capped at 1 MiB, embedded depth was initially capped at 64, and retained
  diagnostics at 10,000. The measured 0.4.2 decision below supersedes that
  initial depth value with 256.
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

- Decision: Version 0.4.x treats the requested part size as a hard serialized
  target, with a 4 GiB default. For each ordinary non-final part, use the
  longest deterministic ordered prefix whose validated PST fits. Extend an
  underfilled trial and reduce an over-limit trial without reordering or
  publishing diagnostic subdivisions. Only the final remainder or the
  indivisible size of the next message may leave a normal part below target;
  only a singleton message that cannot fit may publish as an explicitly marked
  oversize part.
  Rationale: real PST table and tree growth is discontinuous, so a raw-byte
  estimate cannot provide dependable import checkpoints. Calibrating actual
  serialized trials preserves procedural writing while tightly controlling
  completed part sizes.
  Date/Author: 2026-07-17 / Codex after owner correction and focused review.

- Decision: A published part consists of an immutable PST plus a schema-1.0.0
  JSON sidecar whose exact typed value is stored in the ledger transaction.
  Publication intent is durable before rename; reconciliation accepts only the
  exact expected PST/sidecar identities and then commits all item assignments
  atomically. Validator failure evidence contains status and byte counts, never
  validator text that may disclose mailbox data.
  Rationale: a crash at any publication boundary must leave either independently
  valid immutable artifacts that can be reconciled or discardable private
  scratch state, without internally trusting producer reports.
  Date/Author: 2026-07-16 / Codex.

- Decision: Version 0.4.0 does not reinterpret source properties at or above
  `0x8000` when libpff cannot provide their GUID/name identity. It omits them,
  increments structured omission counts, and marks the containing output
  partial. The writer continues to rebuild valid deterministic NAMEID maps for
  typed identities supplied at its boundary.
  Rationale: named-property numeric IDs are local to one store. Guessing an
  identity would produce apparently valid PSTs with silently corrupted custom
  semantics; reopening a damaged source through an unsupervised second parser
  path also conflicts with the established recovery boundary.
  Date/Author: 2026-07-16 / Codex.

- Decision: Message-class admission is ASCII-case-insensitive and respects
  MAPI dot boundaries. Version 0.4.0 accepts `IPM.Note` and its nonempty
  descendants, plus nonempty `REPORT.IPM.Note.<receipt-type>` descendants;
  lookalikes without the separating dot are contained as unsupported.
  Rationale: inconsistent prefix matching could discard legitimate delivery
  reports or admit a malformed class that later poisons an otherwise valid
  part. Cataloging, canonical translation, and writer validation now apply the
  same predicate.
  Date/Author: 2026-07-16 / Codex.

- Decision: A source compressed-RTF property is preserved only after bounded
  header, CRC, token-growth, declared-length, and normative end-reference
  validation. An uncompressed RTF container additionally requires a zero CRC.
  Rationale: candidate-local containment cannot delegate to a decoder that
  considers truncated token EOF successful; malformed body bytes must become
  an explicit property omission instead of risking rejection of the whole PST
  part by an independent importer.
  Date/Author: 2026-07-16 / Codex.

- Decision: Version 0.4.0 preserves source `PidTagMessageFlags` and
  `PidTagInternetCodepage` as typed message inputs. The writer changes only the
  attachment-presence flag to match attachments actually written and uses the
  same value in the message PC and folder contents row. Positive non-UTF-8
  source codepages retain byte-exact HTML; codepage 65001/default HTML must pass
  bounded streaming UTF-8 validation.
  Rationale: replacing read/unread and other state or relabeling source HTML as
  UTF-8 is silent semantic corruption even when the PST remains structurally
  valid.
  Date/Author: 2026-07-16 / Codex.

- Decision: Attachment counts, sizes, payloads, and embedded-item availability
  are untrusted. Count uncertainty and every lookup/stream/size/embedded/limit
  failure are candidate-local partial damage. Non-embedded streaming emits no
  more than the declared byte count, explicit zero-byte payloads receive a real
  durable empty blob, and embedded attachments require an embedded item rather
  than a binary payload.
  Rationale: neither source flags nor native error text proves that an
  attachment table is absent. PSTForge must not convert parser uncertainty,
  oversized streams, or unavailable content into full-success attachment loss.
  Date/Author: 2026-07-16 / Codex.

- Decision: Attachment recovery is isolated per source index. A failed lookup
  emits a placeholder partial attachment; metadata failures default only that
  field; property failures abort only the active property; and data, embedded
  item, or depth failures explicitly abort that attachment before enumeration
  continues. Sink and durable-state failures remain fatal.
  Rationale: one corrupt attachment must not suppress valid later attachments
  or disappear from omission accounting, while a failed ledger write cannot be
  treated as recoverable source damage.
  Date/Author: 2026-07-16 / Codex after clean-context adversarial review.

- Decision: Canonical replay treats attachment metadata, terminal events,
  payload blobs, and embedded-child ownership as one typed state machine. It
  rejects duplicate or contradictory terminals, declaration/length mismatches,
  foreign message IDs, binary payloads on embedded attachments, children on
  non-embedded attachments, and child-plus-terminal states. Partial payload
  prefixes remain private spool evidence and are never serialized.
  Rationale: a durable partial stream or inconsistent ledger event must not be
  upgraded into a complete attachment or move bytes between messages during PST
  publication.
  Date/Author: 2026-07-16 / Codex after clean-context adversarial review.

- Decision: Durable embedded ownership is validated globally across writable
  and unsupported candidates before reachability is constructed. Parent keys,
  attachment indexes, message IDs, and embedded paths must agree with their
  redundant metadata, and one attachment slot may have only one child claim. An
  unsupported embedded message becomes an explicit omitted attachment on its
  writable parent; a writable child of an unsupported parent remains promoted
  as recoverable top-level mail.
  Rationale: unsupported classes are source-local omissions, not grounds to lose
  their parent or every later message, while corrupt ownership must not silently
  reparent recovered mail.
  Date/Author: 2026-07-16 / Codex after clean-context adversarial review.

- Decision: Complete and incomplete property events validate owner kind,
  current message ID, index nullability, and referenced recipient/attachment
  existence. Attachment terminal events independently validate their redundant
  message ID. The private ledger is integrity-checked for contradictory durable
  structure but is not represented as cryptographically authenticated against a
  coherent rewrite of every authoritative and redundant field.
  Rationale: detectable corruption and cross-object reassignment must fail
  closed without overstating what duplicate mutable SQLite fields can prove.
  Date/Author: 2026-07-16 / Codex after clean-context adversarial review.

- Decision: Version 0.4.0 contains only errors produced by the writer's
  side-effect-free input preflight. It bisects a multi-candidate part to isolate
  the source item and durably marks an irreducible singleton unsupported;
  generic construction, completed-output self-validation, independent-reader,
  I/O, and publication failures remain fatal. The preflight uses the writer's
  actual hierarchy/table/property shape checks and bounds aggregate message and
  attachment sizes before output begins.
  Rationale: one damaged message must not block later recoverable mail, but a
  writer defect must never be mislabeled as unsupported source content.
  Date/Author: 2026-07-16 / Codex after clean-context adversarial review.

- Decision: When the writer intentionally omits attachments nested below one
  embedded-message level, every omitted descendant candidate key is marked
  unsupported after the staged part survives completed-store validation and before
  the accepted part is published. Written item assignments include only mail
  actually serialized.
  Rationale: nested descendants must not remain `spooled`, be reported written,
  or be mutated during a failed packing attempt; final written plus unsupported
  accounting must equal every committed candidate.
  Date/Author: 2026-07-16 / Codex after clean-context adversarial review.

- Decision: Preserve source folder placement for Deleted Items and do not move
  deleted mail into Inbox to compensate for MailPlus omitting that folder on
  import. Acceptance compares the generated PST's complete contents separately
  from the target importer's visible result.
  Rationale: flattening or reclassifying recovered mail would change source
  semantics and conceal importer behavior. Outlook confirms the accepted PST
  still contains the message in the correct folder.
  Date/Author: 2026-07-16 / Codex with human acceptance evidence.

- Decision: Output visible folder paths omit only identified PST infrastructure
  nodes: the source store root and its IPM subtree. Preserve every other source
  folder name and hierarchy without a recovery wrapper. Map the well-known
  Deleted Items folder by its source role/NID, never by display name, so an
  ordinary user-created `Deleted items` folder is not conflated with it.
  Rationale: reopening an Outlook-exported PST reproduces its visible hierarchy;
  artificial parents disrupt imports, while names are user data and cannot
  safely identify a well-known folder.
  Date/Author: 2026-07-17 / project owner and Codex.

- Decision: Use the visited native-identifier set only for nonzero top-level
  messages. Never suppress an embedded candidate because its local NID matches
  a top-level message or an embedded message in another subnode tree. Durable
  embedded identity uses parent candidate ownership plus attachment path, and
  the existing maximum embedded depth contains malformed cycles.
  Rationale: NIDs are scoped within subnode trees; global deduplication can turn
  a valid embedded attachment into an unresolved child and abort publication.
  Date/Author: 2026-07-16 / Codex after clean-context adversarial review.

- Decision: Treat the owner's 19 GB corrupt PST as the first large-file
  milestone qualification and the 50 GB corrupt PST as the final scale gate.
  Test in place from the external corpus manifest, write only beneath a
  separate output job directory, and do not create a full source copy merely
  for convenience.
  Rationale: PSTForge opens and rechecks the held source read-only; avoiding
  redundant 19 GB and 50 GB copies reduces test time and disk demand without
  weakening source safety.
  Date/Author: 2026-07-16 / project owner and Codex.

- Decision: Require three times the source size as free output capacity before
  starting a fresh split. A matching resume credits the validated allocation
  already consumed by that job against the same conservative total. Report
  invocation time, logical source/final output bytes,
  end-to-end throughput, and maximum sampled RSS across the supervisor and
  parser workers. Retain empty private directories and the SQLite ledger after
  cleaning spool payloads so completed jobs can be validated and resumed
  without reparsing.
  Rationale: The conservative capacity check covers private spool plus output
  and temporary publication overhead; durable aggregate metrics preserve
  useful qualification evidence after private payload cleanup.
  Date/Author: 2026-07-16 / Codex.

- Decision: Keep job schema version 5 and migrate it additively with an
  `inline_blobs` table, a partial `candidate_events(blob_sha256)` index, and a
  candidate occurrence index over provenance and source identifiers.
  Store properties through 64 KiB transactionally in SQLite, retain streamed
  content-addressed files for larger values, and expose verified inline values
  to the writer through disposable Linux memfd or private cache files.
  Candidate transactions may batch at most 128 completed candidates while each
  candidate remains isolated by a savepoint. SIGINT/SIGTERM commits completed
  work in the batch; SIGKILL or power loss may replay only that current batch.
  Rationale: This removes per-property filesystem durability amplification and
  quadratic resume validation without invalidating the owner's existing
  schema-5 job. The threshold bounds SQLite values and memory, while the batch
  bound makes forced-termination replay finite and testable.
  Date/Author: 2026-07-17 / Codex after the first 19 GB qualification attempt.

- Decision: When `--keep-work` is false, enable SQLite secure deletion, remove
  blob references and payloads transactionally, truncate WAL, vacuum the
  ledger, and checkpoint again. Record compaction as pending in the deletion
  transaction and clear it only after vacuum succeeds, so interruption or
  process death resumes cleanup rather than silently skipping it. Long SQLite
  work uses an interrupt handle monitored from the shared signal flag. Create
  independent-reader extraction only
  inside the held private publication directory. The split writer observes the
  shared interruption flag between messages and blocks, and kills an active
  validator process group when the flag is set.
  Rationale: Inline storage must not turn the retained resume ledger into an
  undeclared archive of recovered content or permanent disk allocation.
  Validator output is generated data and remains subject to the job-directory
  boundary. Installing a signal handler is insufficient unless long output and
  conformance stages can observe it promptly.
  Date/Author: 2026-07-17 / Codex after clean-context adversarial review.

- Decision: Treat 20 minutes as the operational acceptance target for the
  owner's 19 GB qualification on this host, in addition to the existing 2 GiB
  RSS and correctness gates. Emit phase progress during recovery so the absence
  of finalized parts before traversal completes is distinguishable from a
  stall.
  Rationale: The utility is needed for immediate recovery work, and a nominal
  24-hour ceiling does not satisfy the owner's stated usable turnaround.
  Date/Author: 2026-07-17 / project owner and Codex.

- Decision: Close 0.4.1 on the accepted r11 split and validation evidence even
  though its measured 5,317,328,896-byte peak RSS exceeds the original 2 GiB
  qualification objective. Preserve the 2 GiB objective for later optimization
  and release-scale gates. Reserve version 0.4.2 for a separately planned
  data-correctness milestone; do not fold attachment-fidelity changes into the
  reviewed 0.4.1 branch.
  Rationale: The owner confirmed that the milestone's useful outcome is proven:
  the 19 GB source splits within the operational time target into tightly
  controlled, independently valid PSTs whose source-visible folders attach in
  Outlook as expected. Content omissions need focused correctness requirements
  and evidence rather than reopening the validated splitting implementation.
  Date/Author: 2026-07-17 / project owner and Codex.

- Decision: Propagate the shared interruption flag through canonical catalog
  reconstruction, indexed event/ownership reads, content verification, and
  writer-input translation. Launch each independent reader through a hidden
  PSTForge validator supervisor in a dedicated process group. The wrapper arms
  a Linux parent-death signal, verifies its expected parent after arming, and
  kills its complete process group if the parent disappears.
  Rationale: The qualification job can spend meaningful time before and after
  PST serialization. SIGTERM must be bounded in every data-dependent phase,
  while SIGKILL must not orphan a reader or reader descendant that holds the
  private validation scratch tree.
  Date/Author: 2026-07-17 / Codex after clean-context adversarial review.

- Decision: Replace the large-run payload hot path with a single append-only
  job-local pack file. Store checked offset, length, and SHA-256 references in
  the ledger; fsync the pack before committing a bounded candidate batch; and
  truncate any uncommitted tail during resume. Consume candidate and event
  metadata through one ordered cursor and expose bounded pack slices directly
  to the writer. Publish a part as soon as a deterministic group of completed
  top-level messages reaches the target instead of reconstructing the entire
  mailbox first. Failed or interrupted qualification jobs are deleted after
  bounded evidence is retained; successful parts remain for ScanPST and import
  acceptance.
  Rationale: PST node databases must be rebuilt per part, but message payloads
  need only sequential copying plus integrity checks. A pack preserves durable
  crash recovery without creating a second database-shaped mailbox, hundreds
  of thousands of temporary files, or a full replay before the first
  checkpoint.
  Date/Author: 2026-07-17 / project owner and Codex after the fresh 19 GB run.

- Decision: Treat a reported global libpff parser failure with no active
  recovery unit as a recorded partial-recovery boundary after the productive
  traversal, rather than repeating the immutable mailbox. Continue to retry
  worker startup, crash, stall, and transport failures; isolate an exact
  damaged unit after its first contained failure. Match resume candidates by a
  stable metadata multiset rather than synthetic parser IDs or row position.
  Rationale: The 19 GB source deterministically fails its optional recovery
  index at an out-of-bounds offset only after all readable normal candidates
  are durable. Repeating that result cannot recover data and previously turned
  minutes of useful work into repeated full scans.
  Date/Author: 2026-07-17 / Codex from automated 19 GB qualification evidence.

- Decision: Serialize external table row matrices with ordinary maximum-size
  PST data-tree leaves and stream rows across leaf boundaries in the reader.
  Never shorten a non-final XBLOCK child merely to preserve application-level
  record alignment.
  Rationale: MS-PST defines the data tree as one logical stream. ScanPST
  requires each non-final child at the maximum payload size, and independent
  compatibility takes precedence over PSTForge's previous reader assumption.
  Date/Author: 2026-07-17 / Codex from the rejected part-0001 ScanPST evidence.

- Decision: Pack only complete table rows into each external row-matrix leaf,
  pad every non-final leaf to 8,176 bytes, and ignore the sub-row dead-space
  remainder during reads regardless of its contents. Keep BTH row indices
  logical and contiguous across leaves.
  Rationale: This is the representation accepted by ScanPST's repaired r6
  reference and preserves both valid XBLOCK sizing and row lookup semantics.
  Date/Author: 2026-07-17 / Codex after r6 evidence and clean-context review.

- Decision: Version 0.4.2 job schema 6 moves part JSON into
  `.pstforge/manifests/` and publishes one bounded root `recovery.log`; only PST
  files remain in `parts/`.
  Rationale: Import directories must contain only interchange artifacts, while
  resume still requires private crash-reconciled part metadata. The human log
  replaces the sidecars as the operator-facing omission record.
  Date/Author: 2026-07-17 / Codex from the owner-approved 0.4.2 plan.

- Decision: Build embedded messages with the same recursive writer path as
  top-level messages and support 256 embedded levels consistently at native
  intake and writing. An unsupported message class omits only that attachment
  subtree, not its readable parent.
  Rationale: The former one-level builder deliberately emitted an empty
  attachment table for embedded messages and converted readable nested
  attachments into data loss. A shared bounded path keeps tables, size
  accounting, streaming, and validation uniform. No MS-PST nesting boundary was
  found. The former 64 was unmeasured. The 256-level end-to-end fixture
  overflowed a 4 MiB test stack and passed at 6 MiB and 8 MiB; writer execution
  now receives a controlled 32 MiB stack, over five times the observed passing
  threshold. The parser already uses an explicit pending-work stack. Supporting
  substantially deeper input requires converting the remaining recursive
  canonical, translation, writer, and validation paths to explicit work stacks
  rather than guessing another larger constant.
  Date/Author: 2026-07-17 / Codex from checkpoint-1 implementation evidence.

- Decision: The checkpoint-1 human candidate may be a deterministic nested
  writer fixture; later checkpoints that depend on source interpretation must
  select a named real case through `PSTFORGE_CORPUS_MANIFEST`.
  Rationale: This checkpoint changes writer structure, its parser ownership
  path has a separate event-to-writer integration test, and no external corpus
  manifest is configured on this host. The exception does not permit scanning
  user directories or substituting synthetic data for later source behavior.
  Date/Author: 2026-07-17 / Codex.

- Decision: Treat the all-zero GUID returned by libpff for the reserved MAPI
  name-to-ID selector as `PS_MAPI`; preserve nonzero custom GUID bytes exactly.
  Rationale: MS-PST encodes MAPI and Public Strings as reserved selector values
  rather than entries in the custom GUID stream. Libpff 20231205 returns zero
  through `libpff_name_to_id_map_entry_get_guid` for the reserved MAPI selector.
  The native two-generation regression proves that normalizing zero regenerates
  the same numeric PS_MAPI identity and payload, while a literal custom zero
  GUID is not a valid competing named-property set identity.
  Date/Author: 2026-07-17 / Codex from upstream format documentation and native
  roundtrip evidence.

- Decision: Version 0.4.2 job schema 7 makes named-property GUID/name identity
  part of the durable property contract and refuses resume from schema 6.
  Rationale: A schema-6 candidate may already be spooled with only its transient
  `0x8000+` identifier. Replaying that candidate without reparsing would omit
  recoverable named properties, so compatibility must fail closed rather than
  silently downgrade data correctness.
  Date/Author: 2026-07-17 / Codex from checkpoint-2a adversarial review.

- Decision: Put the complete visible source folder hierarchy in part 0001;
  later parts continue to include only paths required by their assigned mail.
  Rationale: Empty folders have no message candidate that can assign them to a
  part. Writing them once preserves source structure without multiplying empty
  objects across every import checkpoint. Part 0001 already participates in
  procedural size calibration, so folder overhead remains inside the requested
  hard part-size target. A source containing no writable item at all still
  requires a separate empty-store writer checkpoint and is not claimed here.
  Date/Author: 2026-07-17 / Codex from checkpoint-2b implementation evidence.

- Decision: Job schema 8 is the first resume-compatible schema for empty
  visible-folder placement. Reconstruct ancestry from durable `FolderAddress`
  values rather than source NIDs. When damaged input exposes multiple sibling
  folders with the exact same display path, preserve one deterministically,
  prefer the well-known Deleted Items role when applicable, count every
  collapsed folder as not copied, and report partial success.
  Rationale: Source NIDs are untrusted and may collide. A valid PST cannot
  represent multiple same-name siblings through the target folder APIs, so
  silently deduplicating them would falsely report complete preservation.
  Schema-7 output may already contain a part 0001 without empty folders and
  therefore cannot safely resume under the new placement contract.
  Date/Author: 2026-07-17 / Codex from checkpoint-2b adversarial review.

- Decision: Job schema 9 is the first resume-compatible schema for contacts,
  and schema 10 is the first for appointments.
  Rationale: Each older durable catalog can contain a readable item from the
  newly admitted class already marked unsupported. Reusing that classification
  would silently omit the item, so each class-admission checkpoint fails
  closed on the immediately preceding schema.
  Date/Author: 2026-07-17 / Codex from checkpoint-3 and checkpoint-4
  implementation evidence.

- Decision: Job schema 11 is the first resume-compatible schema for meeting
  objects, and the admitted boundary is descendants of
  `IPM.Schedule.Meeting` rather than the non-item root itself.
  Rationale: Schema 10 can contain meeting objects already classified
  unsupported. The dotted family covers requests, responses, updates, and
  cancellations without admitting separator lookalikes or synthesizing
  meeting semantics absent from the source.
  Date/Author: 2026-07-17 / Codex from checkpoint-5 implementation evidence.

- Decision: Job schema 12 admits standalone `IPM.Task`, `IPM.StickyNote`, and
  `IPM.Post` families together, while distribution lists, task
  communications, associated messages, and OLE/document storage remain
  separate checkpoints.
  Rationale: The admitted families require only normal-message class,
  sender-policy, folder-class, and named-property handling already present in
  the writer. Personal distribution lists require synchronized
  multivalue-binary member properties; associated data uses a different PST
  contents table; task communications can contain an embedded Task object;
  and OLE/document payloads have their own storage contract. Combining those
  would conceal materially different integrity risks.
  Date/Author: 2026-07-18 / Codex from checkpoint-6 implementation evidence.

- Decision: Job schema 13 admits only the exact
  `IPM.OLE.CLASS.{00061055-0000-0000-C000-000000000046}` calendar-exception
  class and preserves its attachment-owned recurrence linkage separately from
  generic OLE, document, and associated-message storage.
  Rationale: MS-OXOCAL defines this exact embedded class and its exception
  attachment properties, and the 19 GB source confirms that exact shape.
  Dotted descendants and other OLE GUIDs have no demonstrated equivalent
  contract. `PR_ATTACH_SIZE` for an embedded object is recalculated from the
  output child and is therefore compared semantically rather than byte-for-byte;
  exception properties and binary attachment content remain exact.
  Date/Author: 2026-07-18 / Codex from checkpoint-7 implementation and
  independent libpff roundtrip evidence.

- Decision: The appointment checkpoint proves a standalone, non-recurring
  `IPM.Appointment` before meeting and recurrence families. Preserve the exact
  PSETID_Appointment and PSETID_Common named-property identities and values;
  do not derive calendar fields from subject, body, or display timestamps.
  Rationale: Meetings and recurrence add distinct required state and binary
  recurrence structures. Combining them would enlarge the review and human
  failure surface without improving proof for ordinary appointments.
  Date/Author: 2026-07-17 / Codex from Microsoft MS-OXPROPS/MS-OXOCAL and the
  external libpff roundtrip.

- Decision: Prove the oversize-message exception with an exact serialization
  that excludes catalog-only folders whenever enriched part 0001 has one
  message and exceeds the configured maximum. Refuse conformance if the
  baseline fits.
  Rationale: Folder hierarchy overhead is divisible metadata and cannot justify
  the product's exception for an indivisible message.
  Date/Author: 2026-07-17 / Codex from checkpoint-2b adversarial review.

- Decision: Checkpoint 8 models folder location and message placement as
  independent typed source facts. A folder is beneath either the store root or
  IPM subtree; a message belongs to either the normal or associated contents
  table. Neither fact may be inferred from a display name, message class, or
  libpff's coarse item-type classification. Job schema 14 is the first
  resume-compatible schema for this placement contract and for associated
  recovery units.
  Rationale: The 19 GB source contains a normal `SniffData` item in a store-root
  folder, while libpff exposes associated contents through a distinct API.
  Class-based admission alone would move the former into the visible IPM tree
  and continue to miss the latter. A schema-13 ledger cannot prove it traversed
  associated collections and cannot safely resume as complete.
  Date/Author: 2026-07-18 / Codex from read-only source-ledger and installed
  libpff API evidence.

- Decision: No further PST writer feature may be implemented until every
  existing writer invariant is indexed in `docs/WRITER_CONFORMANCE.md` against
  an authoritative Microsoft Open Specification or Microsoft MAPI document.
  Every future writer change updates that index before implementation and
  includes the exact section/property, implementation symbol, focused test,
  and independent evidence. If Microsoft does not publish the needed
  contract, the behavior remains blocked unless the owner approves a clearly
  labeled empirical exception in this Decision Log.
  Rationale: Checkpoint 8 produced an internally valid, libpff-readable PST
  that ScanPST rejected because the implementation created an associated node
  and table row without the separately documented `MSGFLAG_ASSOCIATED`
  requirement. Reader tolerance cannot prove that all normative relationships
  were implemented.
  Date/Author: 2026-07-18 / human owner requirement after checkpoint-8 r1
  ScanPST failure and r2 acceptance.

- Decision: The conformance audit is non-destructive. An undocumented existing
  output remains implemented while it is classified. Before removing,
  disabling, or narrowing completed behavior, document the available Microsoft
  requirements, empirical interoperability evidence, data-preservation impact,
  and concrete options, then wait for the human owner to decide.
  Rationale: Missing traceability identifies uncertainty; it does not prove
  that completed behavior is wrong or expendable. Silent removal would violate
  the milestone's data-preservation and trust goals.
  Date/Author: 2026-07-18 / human owner clarification before checkpoint-9
  conformance audit.

- Decision: Derived/generated recovery counts are a permanent part of the
  bounded human `recovery.log`. They use typed field categories rather than
  source values or per-item records, aggregate across every message placement,
  and survive resume. Reconstructing a structurally required field does not by
  itself make an otherwise preserved candidate partial.
  Rationale: Operators need to know where output contains recovered facts
  versus policy-generated metadata without creating a private-data log or
  conflating reconstruction with source data loss. The underlying fallback
  values remain separate human decisions supported by later comparative
  evidence.
  Date/Author: 2026-07-18 / human owner decisions for EMP-14 and EMP-15
  remediation.

- Decision: Store PtypString allocations as exact UTF-16LE bytes without a
  trailing NUL despite the generic MS-OXCDATA wording.
  Rationale: The strict-NUL implementation produced visible `_` and `€`
  suffix corruption. The exact-length representation is frozen by a focused
  regression and passed ScanPST, Outlook, and MailPlus, making the proven
  interoperable behavior the most correct result for this conflict.
  Date/Author: 2026-07-18 / human owner acceptance of the proven behavior.

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

Version 0.4.0 is implemented and has passed the final accepted-source full
automated gate at `.agent/test-results/1784248198-full`. Candidate r5 also passed the human
acceptance gate: ScanPST reported no errors, both parts opened independently in
Outlook with the expected folder and unread state, and MailPlus imported all
nine Inbox messages. MailPlus omitted the one Deleted Items message that remains
present and readable in the PST, which is recorded as importer behavior rather
than source loss. One high finding from the final whole-milestone review was
resolved by scoping native-ID deduplication to top-level messages, and the
fresh remediation review returned `CLEAN`. The approved milestone is ready for
commit and integration. The current candidate deterministically
converts the durable spool into independently valid size-limited Unicode PSTs,
publishes immutable
part/sidecar pairs through crash-reconcilable intents, and binds every written
candidate to exactly one ledger part. A real healthy Unicode PST produces two
sub-2-MiB parts with exact source-to-output fingerprints for all content libpff
can recover and identical bytes across runs. Libpff attachment-count
uncertainty, unsupported source named properties, malformed values, nested
attachment losses, and attachments beyond the PST signed-size boundary are
explicit partial omissions rather than silent success. The 50 GB damaged
source, interruption/resume, disk preflight, and stale-scratch cleanup remain
0.4.1 qualification work.

Version 0.4.1 implementation now provides exact compatible resume, durable
recovery-completion reuse, continued part numbering, private-work retention or
cleanup, capacity preflight, privacy-safe progress, and bounded runtime/resource
metrics. Focused real-PST tests prove that completed resume does not restart
libpff or change part hashes, configuration mismatch is read-only, SIGTERM and
SIGKILL jobs resume, and a parser worker cannot outlive its supervisor. The
first 19 GB attempt exposed severe property-spool and ledger-validation
amplification. The remediation reduced the representative 2,178-message
real-PST split from more than 334 seconds without completion to 7.83 seconds,
and completed-job resume to 1.93 seconds. The last complete automated gate
passes at `.agent/test-results/1784266394-full`; subsequent containment changes
make canonical prefiltering interruptible and prevent a validator process tree
from surviving supervisor death. Their fast gate passes at
`.agent/test-results/1784268053-fast`; the refreshed full gate and fresh
clean-context review remain before the owner resumes the 19 GB job. The 50 GB
source remains the final 1.0 scale gate.

The rejected 19 GB qualification completed in 9:35.13 and produced 14 finalized
parts totaling 19,128,924,160 bytes. It wrote 36,369 of 37,373 durable
candidates, explicitly accounted for 1,004 unsupported candidates, verified
the source unchanged, and removed the 11 GB private payload pack. Completed
store validation forced four deterministic group bisections instead of
aborting completed work. ScanPST rejected part 0001 because external row
matrices used invalid shortened non-final XBLOCK children; it then orphaned
4,241 messages after losing both folder contents tables. The writer and reader
now follow the logical byte-stream rule and the focused fast gate passes, but
the corrected 19 GB qualification must be repeated. The automated scale result
is also not yet a milestone pass because peak RSS was 5.32 GB, above the 2 GiB
gate; bounded canonical streaming and a fresh full gate/review remain.

The corrected qualification completed in 9:34.50 with the same deterministic
candidate and part accounting. Every part passed `pffinfo` and a complete
one-at-a-time `readpst` extraction without persistent extraction copies. The
source SHA-256 remains unchanged. Its peak RSS was 5,195,616 KiB, so the 2 GiB
memory gate still fails independently of the pending human ScanPST result.
ScanPST rejected r6 part 0001 because 58 rows crossed external row-matrix leaf
boundaries; the XBLOCK and BBT structures themselves were clean. The reviewed
r7 fix pads block-aligned rows, preserves BTH lookup across the boundary, and
passes the fast gate. The r7 qualification completed in 10:15.36 with all 14
parts accepted by both Linux independent readers and all private extraction
scratch removed. The owner subsequently accepted every r7 original in ScanPST,
Outlook, and MailPlus, then rejected its widely underfilled parts and artificial
`Recovered Folder > Top of Outlook data file` visible hierarchy as product
behavior.

The r9 qualification corrected both issues. It preserves source-visible paths
without the store/IPM infrastructure wrapper, distinguishes the well-known
Deleted Items folder from an ordinary same-named user folder by role rather
than text, and procedurally calibrates each deterministic ordered prefix against
its validated serialized size. It completed in 9:54.11 with four normal parts
within 20,175,872 bytes of 4 GiB and one 1,977,541,632-byte final remainder.
All five passed completed-store validation, `pffinfo`, and complete `readpst`
extraction before publication; private payload and extraction scratch were
removed. ScanPST then exposed two shared-block reference-count defects in r9
and r10. The reviewed r11 correction completed in 10:40.29 with the same five
tightly controlled part sizes, exact physical/header EOF agreement, unchanged
source identity, complete independent-reader validation, and removal of
private work. ScanPST runs completed at owner acceptance were clean, and an
original part attached in Outlook with the expected source-visible folders.
The owner accepted that evidence as completion of 0.4.1. Peak RSS remains
5,317,328,896 bytes by explicit exception; attachment/content omission analysis
is reserved for the separately planned 0.4.2 milestone, and the 50 GB source
remains the final release-scale gate.

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

Run the owner's 19 GB corrupt PST first in balanced mode on the current host,
then retain the 50 GB source as the final scale gate. Capture bounded evidence
under `.agent/test-results/`, interrupt normally and with SIGKILL, resume,
validate every part with ScanPST first and the automated independent readers,
and verify source identity. Milestone acceptance requires the 19 GB run to
complete within the owner's 20-minute operational target, with no loss of
finalized parts and final accounting for every discovered mail candidate. The
owner accepted the measured 5,317,328,896-byte peak RSS as a known 0.4.1
limitation; the 2 GiB objective remains for later optimization and release-scale
qualification. The 1.0 release still requires the same acceptance behavior on
the 50 GB source.

The human qualification uses the external source in place; it never copies the
source into the repository or job. Set `SOURCE` to the 19 GB manifest entry and
`JOB` to a new directory on a filesystem with at least three times the source
size free (at least 57 GB for a 19 GB source). Record only hashes, counts,
timings, and paths redacted to corpus case names under `.agent/test-results/`.
Run the release binary in balanced mode, interrupt the first invocation after
the ledger has durable candidates, and resume the same job:

    cargo build --release --locked
    target/release/pstforge info "$SOURCE" --json > "$EVIDENCE/source-info.json"
    /usr/bin/time -v -o "$EVIDENCE/interrupted.time" \
      target/release/pstforge split "$SOURCE" --output "$JOB" \
      --max-pst-size 4GiB --json > "$EVIDENCE/interrupted.json"
    /usr/bin/time -v -o "$EVIDENCE/resumed.time" \
      target/release/pstforge split "$SOURCE" --output "$JOB" \
      --max-pst-size 4GiB --resume --json > "$EVIDENCE/resumed.json"

Use SIGTERM for the normal interruption. The automated external gate covers
SIGKILL and proves the parser child receives the Linux parent-death signal; a
human SIGKILL rehearsal is optional on the 19 GB source unless the automated
gate fails on this host. After completion, compare the final source identity
with `source-info.json`, verify every part hash and sidecar, and run the full
independent-reader gate. On Windows, run ScanPST against every part before
opening any part in Outlook. Record whether ScanPST reports clean, optional
minor inconsistencies, repairable errors, a fatal error, or a crash; preserve
logs and repaired comparisons outside the repository. Only after ScanPST is
complete, open each original part independently in Outlook, then import each
into the dedicated MailPlus test mailbox and compare folder/message counts,
unread/deleted placement, sampled bodies, and attachments.

### Milestone 9: Version 0.4.2 - Incremental Data Correctness

Preserve all readable native PST data that Outlook, MailPlus, or another PST
consumer could use. No item class is excluded merely because MailPlus does not
display it. Full success requires zero semantic omissions; corruption,
unreadable values, unusable source-store references, or legal PST output limits
produce partial success and a plain-language explanation.

Implement on `milestone/v0.4.2-data-correctness` through checkpoint commits
that all retain version 0.4.2. Before each commit, run focused automation,
produce one bounded PST containing the affected data type, finish a fresh
checkpoint review, and stop for ScanPST followed by Outlook. Use a real named
external-corpus case whenever the checkpoint depends on source behavior; a
deterministic writer fixture is sufficient only when parser ownership is
separately covered and the checkpoint changes writer structure. Fix a rejected
candidate without committing. After human acceptance, update this ExecPlan in
the same code commit and push the milestone branch. Do not create progress-only
commits or merge checkpoints into `main`.

Checkpoint order is recursive embedded attachments; lossless native intake;
contacts; appointments; meeting objects; distribution lists; tasks; notes and
post families; OLE, documents, and reference attachments; associated and
configuration data; then remaining generic classes. Private `xtask`
qualification commands write one bounded candidate part without adding public
CLI filtering. Commands that consume real PSTs read only a named case from
`PSTFORGE_CORPUS_MANIFEST`; they never scan user directories.

New jobs keep `parts/` PST-only. Private JSON manifests live beneath
`.pstforge/manifests/`. One mode-`0600` job-root `recovery.log` is atomically
regenerated from durable state and groups exact preserved, relocated, and
unpreserved totals by source-visible folder and human reason. It excludes
subjects, addresses, filenames, payloads, property tags, internal keys, and
native error jargon. Exact totals are never capped; folder detail is bounded to
10,000 lines and 4 MiB.

For checkpoint 1, run:

    cargo xtask qualify embedded-attachments \
      /home/mbeutler/.local/share/pstforge/acceptance/qualification-v042-embedded-r3

The output directory must be new, absolute, and outside the repository. The
command stages the directory beside its destination and publishes it only
after completed-store, `pffinfo`, and `readpst` validation. Scan only
`parts/part-0001.pst`; then open that same unrepaired PST in Outlook and inspect
both nested attachment levels.

For checkpoint 2a, the private source PST must remain outside the repository
and be selected by the `v042-named-property-source` case in
`PSTFORGE_CORPUS_MANIFEST`. Run the ignored external-corpus regression to prove
the libpff-to-writer roundtrip, then inspect only the bounded native output:

    PSTFORGE_CORPUS_MANIFEST=/absolute/private/manifest.toml \
      cargo test -p pstforge-cli --test external_corpus --locked \
      milestone_0_4_2_named_properties_roundtrip_through_libpff \
      -- --ignored --exact

    scanpst qualification-v042-named-r1/parts/part-0001.pst

After ScanPST, open the unrepaired original candidate in Outlook and confirm the
single `Named property fidelity checkpoint` message opens normally. Named
properties are not asserted through Outlook's visible UI; the exact independent
libpff comparison is the semantic acceptance evidence.

For checkpoint 2b, run the ignored
`milestone_0_4_2_empty_folders_roundtrip_through_libpff` external-corpus test,
then scan only
`qualification-v042-empty-folders-r1/parts/part-0001.pst`. After ScanPST, open
the unrepaired candidate in Outlook. Confirm `Inbox` contains the single
`Empty folder fidelity checkpoint` message; `Deleted Items`, the ordinary
case-distinct `Deleted items`, `Empty Parent`, and its nested `Empty Child` are
all visible; and no artificial recovery or source-store wrapper folder exists.

For checkpoint 3, select external case `v042-contact-source`, run ignored test
`milestone_0_4_2_contacts_roundtrip_through_libpff`, and scan only
`qualification-v042-contact-r1/parts/part-0001.pst`. Then open the original,
unrepaired candidate in Outlook. Confirm the `Contacts` folder uses the contact
view and opens one `Ada Lovelace` contact with the expected name, company,
title, business phone, mobile phone, birthday, email address, File As value,
and notes. The independent libpff comparison remains the exact property
fidelity evidence.

For checkpoint 4, select external case `v042-appointment-source`, run ignored
test `milestone_0_4_2_appointments_roundtrip_through_libpff`, and scan only
`qualification-v042-appointment-r1/parts/part-0001.pst`. Then open the
original, unrepaired candidate in Outlook. Confirm `Calendar` uses the calendar
view and contains one `Appointment fidelity checkpoint` appointment on
January 15, 2025 from 10:00 AM to 11:00 AM America/Detroit (15:00-16:00 UTC),
located in `Conference Room 42`, shown as Busy, with a 15-minute reminder,
non-recurring status, and the expected notes.

For checkpoint 5, select external case `v042-meeting-source`, run ignored test
`milestone_0_4_2_meetings_roundtrip_through_libpff`, and scan only
`qualification-v042-meeting-r1/parts/part-0001.pst`. Then open the original,
unrepaired candidate in Outlook. Confirm Inbox contains one
`Meeting request fidelity checkpoint` request from `PSTForge Organizer` to the
attendee, January 15, 2025 from 10:00 AM to 11:00 AM America/Detroit
(15:00-16:00 UTC), located in `Conference Room 42`, shown as Busy, with the
15-minute reminder and expected notes. Confirm Outlook presents meeting
request controls without an item-corruption warning; do not send a response.

For checkpoint 6, select external case `v042-pim-source`, run ignored test
`milestone_0_4_2_pim_items_roundtrip_through_libpff`, and scan only
`qualification-v042-pim-r1/parts/part-0001.pst`. Then open the original,
unrepaired candidate in Outlook. Confirm the `Tasks` folder is a normal Tasks
folder containing `Task fidelity checkpoint`, with start and due dates, zero
percent complete, incomplete state, and the expected notes. Confirm the
`Notes` folder is a normal Notes folder containing
`Sticky note fidelity checkpoint`, with yellow color, a usable note window,
and the expected body. Confirm the `Posts` folder contains
`Post fidelity checkpoint` from `PSTForge Poster`, opens as a Post item, and
shows the expected body. Stop and provide the ScanPST log if ScanPST reports
any error or requests repair.

For checkpoint 7, select external case
`v042-calendar-exception-source`, run ignored test
`milestone_0_4_2_calendar_exceptions_roundtrip_through_libpff`, and scan only
`qualification-v042-calendar-exception-r1/parts/part-0001.pst`. Then open the
original, unrepaired candidate in Outlook. Confirm the `Calendar` folder opens
normally and the `Recurring appointment exception checkpoint` appointment can
be opened without an item-corruption warning. The bounded fixture proves the
hidden exception object and its attachment-owned linkage through exact libpff
fingerprints; it does not fabricate a complete recurrence pattern for visible
calendar-instance testing. Stop and provide the ScanPST log if ScanPST reports
any error or requests repair.

After every available focused checkpoint succeeds, run the complete 19 GB
split once. Unique source items assigned across all output parts must equal the
readable source inventory, or the recovery record must establish high
confidence by exactly reconciling readable items, failed source slots,
recovery-collection boundaries, written fingerprints, and explicitly
unwritten content. Validate all final parts with independent readers, ScanPST,
Outlook, and MailPlus. The 50 GB source remains a later release gate.

### Milestone 10: Version 0.5.0 - Operational UX and Debian Packaging

Finalize report schemas, privacy redaction, exit statuses, error wording,
report regeneration, install diagnostics, spool cleanup, and operator docs.
Create a reproducible Debian 13 x86_64 `.deb` that dynamically depends on
`libpff1t64` at the Debian-compatible floor. Run build and integration tests in
a clean Debian 13 environment and on Ubuntu 26.04.

Acceptance: installing the package on clean Debian 13 makes `pstforge info`,
`verify`, `split`, and `report` work with only declared dependencies; removing
it leaves user jobs and source PSTs untouched; package contents, licenses, and
dependency metadata pass inspection.

### Milestone 11: Version 0.5.1 - GitHub CI and Private-Corpus Automation

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

### Milestone 12: Version 0.6.0 - Interoperability Release Candidate

Freeze CLI/schema changes, run every local and GitHub gate, fuzz parsers and
writer structures, inject disk and process failures, and complete security,
license, privacy, and data-loss reviews. Import representative parts into a
clean MailPlus test mailbox and compare folder/message counts and sampled
content. Open the same parts in supported Outlook as a secondary test.

No blocker or high-severity review finding may remain. Medium findings must be
fixed or explicitly accepted by the human owner in the Decision Log. A release
candidate is not 1.0 until the real 50 GB rehearsal succeeds from clean state.

### Milestone 13: Version 1.0.0 - MailPlus-Ready Release

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
