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
mail candidates. With `--restartable`, stopping and rerunning with `--resume`
continues compatible work. Hash and identity evidence show that the source was
not modified.

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
- [x] Milestone 0.4.2: Incremental Data Correctness.
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
    - [x] (2026-07-18) Separated the mixed EMP-02 classification without
      changing output. MS-PST 2.7.3.3 explicitly mandates six empty
      table-template objects at their section-specific NIDs; record their
      presence and emptiness as verified MSG-13 behavior, four exact schemas,
      and the existing EMP-06 hierarchy/contents exceptions. Narrow EMP-02 to
      the three additional
      ScanPST-derived index nodes whose fixed NIDs, schemas, and reserved type
      bits remain undocumented. The first review rejected an overbroad schema
      claim because hierarchy/contents types remain EMP-06; the final wording
      preserves those exceptions and verifies only the supported scope. One
      full-gate run hit the unchanged SIGKILL timing test before SQLite schema
      initialization; its focused rerun passed, and the repeated complete gate
      passed at `.agent/test-results/1784394282-full`. A fresh final-state
      review returned `CLEAN`. No production code, PST bytes, or human
      acceptance behavior changed.
    - [x] (2026-07-18) Resolved the structural empirical-output checkpoint
      without changing writer bytes. The owner accepted EMP-01 through EMP-10
      as required interoperability behavior: current output passes ScanPST and
      Outlook, historical omissions produced repair findings, and no
      authoritative alternative has demonstrated better real-world behavior.
      The same evidence rule resolves EMP-13 in favor of exact source
      preservation because its source/output properties match and the result
      passes ScanPST and Outlook. The final combined-manifest full gate passed
      at `.agent/test-results/1784395034-full`; this checkpoint changes only
      conformance and decision documentation. The first clean-context review
      rejected `Verified` on grouped rows that also use accepted exceptions;
      define `Accepted mixed conformance` so their verified normative portions
      and approved empirical/conflicting portions remain distinguishable.
    - [x] (2026-07-18) Accepted the source-derived portion of EMP-14 and EMP-15
      without changing writer bytes. Folder class from message class, the
      readable half of sender and recipient identity pairs, valid delivery time
      for creation/modification time, and associated display name from subject
      remain permanent recovery policy. Each derivation retains a readable
      source fact, remains separately counted in `recovery.log`, and never
      authorizes an unrelated generated value. The combined-manifest full gate
      passed at `.agent/test-results/1784395507-full`.
    - [x] (2026-07-18) Corrected two reconstruction provenance categories
      without changing PST bytes. A missing Internet code page is derived as
      65001 only when the nonempty complete HTML stream passes strict UTF-8
      validation; absent, empty, or invalid HTML leaves the existing default
      classified as generated. An absent MIME tag on an actual method-5
      embedded Message object is derived from that structure rather than
      classified as a generated byte-signature guess. Focused tests freeze both
      boundaries.
      The combined-manifest full gate passed at
      `.agent/test-results/1784396492-full`; its independent writer-acceptance
      log is byte-for-byte identical to the preceding approved checkpoint at
      `.agent/test-results/1784395507-full`.
    - [x] (2026-07-18) Implemented the owner-directed bounded MIME-signature
      checkpoint for missing by-value attachment MIME. A valid source value
      still wins. Complete payloads can derive only PDF, PNG, JPEG, GIF, or
      classic TIFF from exact format-defined leading signatures after at most
      eleven verified bytes; ZIP, OLE, text, short prefixes, and unknown data
      remain unlabeled. The combined-manifest full gate passed at
      `.agent/test-results/1784397423-full`. The first review correctly rejected
      a three-byte JPEG marker as insufficient; the final implementation
      requires the complete JFIF APP0 identifier and its actual packed-blob
      negative test. A fresh final-state adversarial review reports CLEAN.
      The human candidate is
      `qualification-v042-mime-r1/part-0001.pst`, 271,360 bytes, SHA-256
      `0f856d1a59bb1d56c56cb77b6608e0bc6edaa2fdeb6f12c5c6e62af198e4095c`.
      `pffinfo` accepts it, and `readpst` exports `application/pdf` with the
      exact source payload SHA-256
      `99b86ad90b88183998f68fb68bac449b56be3a6a5ba8cd5aba299288bb4eb480`.
      The final combined-manifest full gate passed at
      `.agent/test-results/1784398030-full`. The owner reports ScanPST and
      Outlook acceptance clean, approving this output-changing checkpoint for
      commit and push.
    - [x] (2026-07-18) Extended missing attachment-type recovery to common
      emailed containers without changing payload bytes. Exact ZIP signatures
      produce the generic `application/zip` label; bounded OPC parsing upgrades
      only an exact `[Content_Types].xml` override plus matching required main
      part to DOCX, XLSX, or PPTX. Pre-bounded CFB parsing recognizes DOC, XLS,
      or PPT from one unambiguous root main stream plus format markers.
      Because source filename and MIME are independent MAPI properties, a
      recognized extension can refine generic ZIP or a matching single-family
      CFB, but never arbitrary bytes or a conflicting proven subtype. Parser
      failure, cross-wired metadata, nested evidence, and conflicting subtypes
      degrade to generic ZIP or unknown rather than dropping the attachment. An
      unknown attachment with no nonempty source filename is exposed as
      `Recovered attachment {index}.bin`; this renames no source and converts
      no bytes. `docs/ATTACHMENT_RECOVERY.md` is the release-facing confidence
      and corruption-limit matrix. The first fresh review found three
      applicable gaps: CFB directory allocation preceded the entry cap, OPC
      namespace/duplicate evidence was under-validated, and legacy root-stream
      names alone did not prove their Office family. The implementation now
      preflights CFB FAT chains and allocation counts before `cfb::open_with`,
      requires a unique namespace-correct OPC package, validates DOC FIB, XLS
      BOF, and PPT main/current-user record markers, and tests every rejected
      ambiguity. The repeated full gate passed at
      `.agent/test-results/1784400359-full`, and the fresh final-state
      adversarial review reported no applicable blocker, high, or medium
      findings. The focused attachment checkpoint at
      `qualification-v042-attachment-types-r2` passed independent `pffinfo` and
      `readpst` validation; the owner reports ScanPST and Outlook acceptance
      clean.
    - [x] (2026-07-18) Resolved the neutral attachment metadata portion of
      EMP-15 without changing PST bytes. When the source omits rendering
      position, `-1` documents that the plain-text body-position property does
      not render the attachment; RTF retains its separate placeholder
      mechanism. When the source omits attachment flags, zero is the documented
      process-in-all-applications value. The packed-blob recovery regression
      freezes both translated inputs, their bounded generated-value accounting,
      and completed PST serialization. Generated display filenames other than
      the accepted unknown `.bin` case remain a separate owner decision. The
      combined-manifest full gate passed at
      `.agent/test-results/1784401533-full`; its writer-acceptance evidence is
      byte-identical to the preceding output-changing checkpoint. The first
      focused review corrected overbroad rendering terminology, required
      completed-PST validation, and corrected the source page date. A fresh
      final-state review returned `CLEAN`.
    - [x] (2026-07-18) Generate a correct deterministic extension when a
      readable attachment has no nonempty source filename. Preserve every
      source filename unchanged. For a generated name, payload-proven type
      evidence outranks conflicting source MIME metadata, a recognized
      preserved source MIME supplies the extension when content is
      inconclusive, and all remaining by-value data uses `.bin`. Embedded
      Message objects retain `.msg`. Supported mappings are bounded to the
      already documented PDF, image, ZIP, and Office formats; filename
      generation does not change payload or MIME bytes. Focused tests cover
      mapping, MIME parameters/case, ambiguity, provenance counts, conflict
      precedence, and completed PST validation. The combined-manifest full gate
      passed at `.agent/test-results/1784402171-full`. The first focused review
      required end-to-end coverage of the recognized-source-MIME fallback; the
      remediated test publishes and validates that case, and a fresh final-state
      review returned `CLEAN`. The focused candidate
      `qualification-v042-generated-extensions-r1/parts/part-0001.pst` is
      271,360 bytes with SHA-256
      `544b37b9e862aa92442298c874f3afab05a99a4a9e6bd0b9d8f3117fabcab0f5`;
      `pffinfo` and `readpst` accept it with `.pdf`, `.bin`, and `.docx`
      generated names. The owner reports ScanPST and Outlook acceptance clean.
    - [x] (2026-07-18) Compared current fabricated visible metadata with an
      integrity-first omission before deciding EMP-14 subject/sender policy.
      One PST contains otherwise equivalent messages: comparison A writes
      `(no subject)` and `Unknown Sender`; comparison B omits an unknown subject
      and all sender identity properties. The writer continues to reject a
      one-sided sender pair. Empty Unicode table values are not emitted because
      they encode as invalid HID zero; the contents-table column remains and
      its existence bit is clear. The focused writer test proves completed
      readback. Candidate
      `qualification-v042-missing-metadata-comparison-r2/parts/part-0001.pst`
      is 271,360 bytes with SHA-256
      `d3381fcad5f7a31b9113112d77a32246e7b4ce6919364de63df91c65a88bb85a`;
      `pffinfo`, `readpst`, and ScanPST accept both messages. Outlook leaves
      omitted fields blank in the list and supplies `(no subject)` only in the
      opened view. MailPlus supplies `(No subject)` for the omitted subject and
      leaves the omitted sender blank, while the fabricated sender renders as
      `<Unknown@SYNTAX_ERROR>`. The owner therefore selected omission and
      client-controlled presentation. Production translation now leaves wholly
      missing subject and sender identity absent across readable message
      classes while retaining typed bounded missing-metadata counts in
      `recovery.log`. Associated messages may omit subject but retain their
      separately validated nonempty display-name rule; when both source display
      name and subject are absent, the prior neutral `(no subject)` fallback is
      retained only as the associated-table display name and counted there.
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
    - [x] (2026-07-18) Checkpoint 10: Personal Distribution Lists. Admit
      `IPM.DistList` descendants only after implementing the MS-PST
      variable-width multivalue envelope and the MS-OXOCNTC synchronized
      member-property contract. Preserve structurally readable PSETID_Address
      LID `0x8055` and optional `0x8054` values byte-for-byte, require each
      encoded property to remain below 15,000 bytes, and never synthesize,
      reorder, reinterpret, or checksum-rewrite recovered members. A missing
      one-off mirror remains absent as permitted by documented Exchange 2003
      behavior. A malformed, oversized, or count-mismatched mirror is omitted
      alone and reported partial; an unusable primary list omits the
      member properties while retaining recoverable message metadata.
      Job-schema compatibility must advance because schema 15 durable
      catalogs can already contain these properties classified as omitted.
      Prove the exact source/output property fingerprints with one bounded
      external fixture, then stop after the first generated part for ScanPST
      and Outlook verification before commit.
      Implementation now preserves the exact bounded multivalue-binary
      envelopes, contains each malformed or inconsistent source property
      without aborting the message, derives `IPF.Contact` only when the source
      folder class is absent, and refuses resume from job schema 15. Focused
      tests cover the 14,999-byte accepted boundary, the 15,000-byte rejected
      boundary, malformed offsets, wrong source types, synchronized arrays,
      and exact omission counts. The automated external fixture proves exact
      source/output named-property fingerprints through libpff. The
      combined-manifest full gate passed at
      `.agent/test-results/1784413306-full`, and a fresh final-state
      adversarial review returned `CLEAN`. Candidate
      `qualification-v042-distribution-list-r3/parts/part-0001.pst` is 271,360
      bytes with SHA-256
      `48b2bbde100abeae2051e5dc542cb19479c23c8d0f6880712f01d3d84d5c6d40`;
      `pffinfo` accepts it and `readpst` completes while intentionally skipping
      the non-mail Contact object. The owner reports ScanPST and Outlook
      acceptance fully green, including the Contacts-folder list name and both
      members.
    - [x] Checkpoint 11a: By-value Document objects. Admit only dotted
      `IPM.Document.*` descendants under MS-OXODOC. Preserve every readable
      attachment even though the protocol recommends no more than one, and
      report a Document object with no readable attachment as partial without
      discarding its message metadata. Preserve the exact source file-type
      suffix, `PidTagDisplayName`, attachment metadata and payload, and all
      representable document-specific named properties without deriving them
      from the filename or file contents. Add bounded `PtypMultipleString`
      preservation for `PidNameKeywords` and `PidNameDocumentParts`, contain
      malformed offsets or UTF-16 to the affected property, and advance the
      job schema from 16 to 17. Prove exact source/output message, named
      property, and attachment fingerprints using one bounded DOCX fixture,
      then stop for ScanPST-first Outlook acceptance before commit.
      Implementation now preserves the Document class suffix, display name,
      every readable by-value attachment, and representable Document named
      properties. Bounded multivalue-Unicode decoding rejects malformed
      offsets, invalid UTF-16, and non-NUL data after a terminator; the writer
      uses checked counts, offsets, and lengths. Focused tests cover terminal
      NUL containment, the Public Strings keyword limit, zero/one/multiple
      attachments, and an unreadable sole attachment. The exact external
      libpff source/output comparison reports no omissions and the source
      identity remains unchanged. The combined-manifest full gate passed at
      `.agent/test-results/1784427024-full`, and the fresh final-state
      adversarial review returned `CLEAN`. Candidate
      `qualification-v042-document-r2/parts/part-0001.pst` is 271,360 bytes
      with SHA-256
      `d48b7eebc4936ffd60c739dc5642ab05f5db7734ce14fe0d81614f6483aeb0d7`.
      `pffinfo` accepts it and `readpst` completes while intentionally skipping
      the non-mail Document object. `pffexport` extracts the DOCX from both the
      immutable source fixture and split output with identical SHA-256
      `6189ada04b0f10ed91272485315c5d4d5b90e8a6589fabc145a5b33af8181b33`;
      `unzip -t` verifies ZIP integrity and readability of all three parts,
      while the focused fixture test asserts the package relationship type and
      target. The owner reports ScanPST clean, Outlook displays the Document
      object, and Word opens the DOCX normally.
    - [x] Checkpoint 11b: By-reference and web-reference attachments. Preserve
      documented method/path or URL relationships without fetching external
      content or converting a reference into by-value data. Support OXCMSG
      methods `2`, `4`, and `7`, plus legacy MAPI method `3` as proven Outlook
      reality. Methods remain data-less and require a readable long pathname.
      Preserve optional short pathname and exact PSETID_Attachment provider and
      permission metadata for web references. Never stat, open, resolve, or
      fetch any referenced target. Missing/malformed required relationships
      omit only that attachment and report partial recovery. Advance the job
      schema from 17 to 18, prove exact source/output method, path, NAMEID, and
      absence-of-content fingerprints using one bounded external fixture, then
      stop for ScanPST-first Outlook verification before commit.
      - [x] (2026-07-18) The parser now reads `PidTagAttachMethod` directly
        before invoking libpff attachment-content helpers. Methods `2`, `3`,
        `4`, and `7` are durably classified as data-less references, so the
        parser never requests target size or content and valid references no
        longer create false native read issues. Schema 18 records the
        `attachment_reference` terminal separately from missing payload data.
      - [x] (2026-07-18) Typed canonical and writer models preserve exact
        method, long and optional short path, filename, and web provider and
        permission NAMEID values. Completed-store validation requires no
        `PidTagAttachDataBinary`. Missing long paths omit only the attachment;
        any contradictory readable by-value property is counted as a property
        omission rather than silently discarded, including a zero-byte value.
        Canonical replay retains the distinct reference terminal and will not
        reconstruct an incomplete attachment from surviving path properties.
      - [x] (2026-07-18) The bounded external
        `v042-reference-attachments-source` fixture and split output compare
        exactly through libpff for all relationships and report zero
        omissions. The fixture uses unreachable `.invalid` UNC/URL targets,
        proving the implementation does not depend on their existence.
        `pffinfo` accepts the fixture and `readpst` completes. The complete
        corrected full gate passed at
        `.agent/test-results/1784429473-full`, including every earlier 0.4.2
        external checkpoint. A first clean-context review found the zero-byte
        contradiction and durable-terminal gaps above; both now have focused
        regressions. A fresh final-state clean-context review returned `CLEAN`.
        The owner reports that the r2 candidate works, satisfying the
        ScanPST-first Outlook interoperability gate.
    - [x] Checkpoint 11c: OLE attachments. Preserve source object/binary data,
      attach tag, static rendition, and method relationship without converting
      or executing the object. Method `6` preserves `0x3701` as `PtypObject`
      for OLE 2 storage or `PtypBinary` for OLE 1 OLESTREAM data, selected only
      from the readable source property type. Preserve optional `0x370A`
      attach-tag, `0x3702` encoding, and `0x3709` static-rendition bytes exactly
      when present, including an explicitly empty rendition. Never infer,
      synthesize, execute, repair, convert, or dereference object content.
      Preserve a complete zero-byte `PtypBinary` payload as an exact readable
      source value. Treat zero-byte `PtypObject` as malformed because its object
      descriptor cannot reference a valid empty PST data block. Missing,
      incomplete, oversized, or malformed required payloads omit only that
      attachment and report partial; malformed optional metadata is omitted
      alone. Advance the job schema from 18 to 19 so resumed jobs
      cannot mix pre-OLE and OLE-aware output semantics. Prove exact method,
      property type, payload, tag, encoding, rendition, and absence semantics
      with one bounded external fixture, independently validate its OLE 2
      payload as Compound File Binary, then stop for ScanPST-first Outlook
      verification before commit.
      - [x] (2026-07-19) Located and recorded the normative method-6,
        `PidTagAttachDataObject`, attach-tag, encoding, and static-rendition
        relationships in CLS-12. The recovery boundary preserves source facts
        without making OLE content validity a prerequisite for salvage.
      - [x] (2026-07-19) Added typed method-6 object/binary writer input,
        exact streamed serialization, chunked completed-store hashing, libpff
        object-subnode traversal, streamed optional binary metadata, and schema
        19.
        Missing, incomplete, oversized, or wrongly typed required payloads
        omit only their attachment; malformed optional metadata omits only
        that property.
      - [x] (2026-07-19) The bounded
        `v042-ole-attachments-source` fixture uses both documented `0x3701`
        representations. Its OLE 2 payload is created and reopened by the
        independent `cfb` parser. Exact libpff source/output comparison covers
        method, type, payload hash, tag, encoding, rendition, and explicitly
        empty rendition with zero omissions and unchanged source identity.
        `pffinfo` accepts the source and candidate, and `readpst` completes for
        both. The corrected fast gate passed at
        `.agent/test-results/1784467894-fast`; the complete combined-manifest
        gate passed at `.agent/test-results/1784468121-full`, including every
        earlier 0.4.2 checkpoint and exact retention of a real method-1 JPEG
        attach tag.
        The first candidate reported zero omissions but failed ScanPST because
        its `PtypObject` data subnode used raw-LTP NID type `31`. ScanPST then
        invalidated `PR_ATTACH_SIZE` and cascaded through the attachment and
        contents rows; its repaired reference deleted the OLE 2 attachment
        instead of preserving or relabeling it. The r2 candidate changed the
        object subnode to internal NID type `1`, but ScanPST rejected that type
        as well and its repair again discarded the 2,560-byte object payload.
        A ScanPST-clean Outlook-authored comparison PST resolves the previously
        undocumented relationship: all five method-6 `PtypObject` descriptors
        reference object-data subnodes with reserved NID type `0x09`.
        PSTForge will reproduce that observed Outlook encoding and completed-
        store validation will assert it. The clean-context review also found
        that reference and embedded attachment branches counted streamed
        optional metadata as omitted without removing it from writer input,
        allowing one value above 16 KiB to reject the entire message. That
        containment defect is included in the remediation. The former
        16-KiB optional-metadata materialization bound could omit valid WMF
        renditions; values above that threshold now remain in the immutable
        spool and are streamed and hash-validated. Focused regressions cover a
        20-KiB rendition.
        The initial clean-context review found two medium issues: complete
        zero-byte method-6 data had no evidence-based disposition, and
        preflight sizing omitted inline raw attachment-property payloads. A
        zero-byte PST data block then failed independent reader validation with
        `BLOCKTRAILER cb = 0`; PSTForge therefore preserves complete empty
        `PtypBinary` inline and contains empty `PtypObject` as malformed.
        Preflight sizing now counts raw plus streamed metadata before writing.
        Focused regressions pass, and the pre-r2 combined-manifest full gate
        passes at `.agent/test-results/1784470289-full`. The Outlook specimen
        has SHA-256
        `99fc6e28ca18900f54c9411cbbcd5ef6a29fa2e6e1c5b0fd2e0b573411c15f48`;
        `libpff`, `pffinfo`, and PSTForge accept it, and the owner reports
        ScanPST clean. The Outlook-source roundtrip now preserves all five
        attachment payload hashes plus the exact method, `PtypObject`,
        attach-tag, and encoding fingerprints with no omitted attachments.
        Its enclosing message remains partial for 50 already-visible non-OLE
        property gaps, which are outside this focused writer correction. The
        final clean-context review identified one additional containment gap:
        a near-2-GiB OLE payload could pass translation before its retained
        metadata pushed the aggregate attachment property size over the signed
        PST limit and rejected the parent message. Canonical translation now
        uses the writer's authoritative aggregate-size calculation and omits
        only that attachment. The replacement candidate
        `qualification-v042-ole-r3/parts/part-0001.pst` is 271,360 bytes with
        SHA-256
        `0e46a4a7b0c21b91da9bfd3c5b1df0b4f01d92871e25ea87ffe5d78bd5ac8c76`
        and reports zero omissions. It contains the type-`0x09` object-data
        relationship and a 20-KiB streamed rendition. `pffinfo`, `readpst`,
        `pffexport`, and exact libpff source/output comparison accept it; both
        OLE payloads and the complete optional-metadata fingerprints remain
        exact. The corrected focused tests, fast gate
        `.agent/test-results/1784473337-fast`, and combined-manifest full gate
        `.agent/test-results/1784473375-full` pass. A fresh clean-context review
        found one medium header-counter concern, but the proposed change
        conflicts with the ScanPST-clean Outlook source: its five type-`0x09`
        subnodes advance `rgnid[0x09]` to the same subnode-index high-water
        convention used by PSTForge. The owner reports candidate r3 clean in
        ScanPST, but Outlook could not open its synthetic OLE payloads. That
        fixture intentionally contained a generic CFB storage and arbitrary
        OLE1 bytes, so it remains structural evidence rather than an
        application-openability oracle.
        A real-source r4 roundtrip retained all five exact OLE payloads and
        passed ScanPST, but Outlook displayed an empty draft. Independent
        `pffexport` comparison showed the source's 56,139-byte decoded RTF body
        was absent from r4. Its valid compressed container declares a
        56,138-byte `RAWSIZE` including a final NUL; the decoder removes that
        terminator and returns 56,137 code units. PSTForge's validator required
        equality and therefore omitted the RTF plus its `\objattph` display
        relationships. Validation now accepts exactly one stripped terminal
        NUL while retaining header, CRC, end-run, and size bounds. The real
        Outlook roundtrip regression now requires exact compressed-RTF length
        and hash in addition to the five OLE contracts. The focused regression,
        fast gate `.agent/test-results/1784474293-fast`, and combined-manifest
        full gate `.agent/test-results/1784474319-full` pass. A fresh
        clean-context review returned `CLEAN`. The real-source replacement r5
        candidate is 271,360 bytes with SHA-256
        `6c56871aaf0aed122c3e43516b0afd358ac0178207bcd030a7b6001d12b3744e`;
        `pffinfo`, `readpst`, and `pffexport` accept it, with the 56,139-byte
        decoded RTF and all five exact OLE payload sizes present. The owner
        reports ScanPST clean and confirms Outlook preserves the original
        rendered state exactly. This satisfies the checkpoint interoperability
        gate.
  - [x] (2026-07-19) Close the focused 0.4.2 data-correctness checkpoint
    series after every admitted data type passed its required automated,
    ScanPST, and Outlook evidence.
  - [x] (2026-07-19) Record rather than conceal the incomplete final 19 GB
    split and whole-job reconciliation. The owner stopped the cold run after
    57:47 because only one 4 GiB part had finalized and the writer was
    repeatedly serializing trial parts. This is an explicit performance-gate
    failure deferred to 0.4.3, not a successful 0.4.2 scale result.
- [x] Milestone 0.4.3: Incremental Writer Performance.
  - [x] (2026-07-19) Recorded the failed 0.4.2 cold qualification and created
    `milestone/v0.4.3-performance` from pushed `main` commit `4ebb7dd`.
  - [x] (2026-07-19) Checkpoint 1: expose a transactional writer that appends one complete
    top-level message, projects the exact finalized file extent from retained
    blocks/tables/B-trees, and rolls back only the newest uncommitted message.
    A normal finalized part is serialized once. Byte-equivalence, exact
    projection, individual rollback, and bounded-batch rollback tests pass.
  - [x] (2026-07-19) Checkpoint 2: replace whole-mailbox canonical and writer-input vectors
    with an ordered ledger cursor. Translate, validate, and append one
    top-level candidate tree at a time. The header pass pages 1,024 top-level
    metadata/ownership rows at a time and immediately reduces them to the
    compact deterministic packing index; it never loads source-wide embedded
    ownership or property events. The 19 GB run holds about 321 MiB peak RSS
    and serializes every normal part once.
  - [x] Checkpoint 3: persist the current part transaction boundary and make
    SIGINT/SIGTERM observable throughout canonical reads, blob streaming,
    append, finalization, hashing, and validation. Prove that materially
    progressed resume is faster than cold restart. Retained-spool r4
    republished all five parts in 2:45.09 versus the 7:35.21 cold r3 run,
    without restarting libpff.
  - [x] Checkpoint 4: run the retained-job benchmark, then a fresh named 19 GB
    split. Require one serialization per normal part, completion within one
    minute per source GiB, less than 2 GiB aggregate RSS, exact accounting,
    independent validation, and unchanged source identity.
    Baseline r1 completed in 11:03.95 with first publication at approximately
    6:49. Optimized r2 published part 1 in approximately 5:20 and reduced its
    private append phase from about 104 seconds to 15 seconds, but exposed and
    then reproduced a batch replay defect involving a durably unsupported
    candidate. The focused regression, complete core/writer suites, and strict
    Clippy pass after the fix. Final uninterrupted r3 completed in 7:35.21,
    published part 1 in 5:21, used 320,416 KiB maximum RSS, serialized every
    part once, wrote 37,035 candidates, and kept the source unchanged. Its five
    part lengths, SHA-256 values, message/omission counts, recovery counts, and
    total 19,333,198,848 output bytes are byte-for-byte identical to independent
    baseline r1. Automated performance and deterministic-output requirements
    pass. Canonical combined-manifest full gate
    `.agent/test-results/1784489410-full` passes, including licenses,
    advisories, writer `pffinfo`/`readpst`, and all external corpus cases.
    The first clean-context review found one high candidate-dependent folder
    validation defect and one medium terminal-candidate replay defect. Folder
    layout construction/validation is now message-independent, and terminal
    candidate keys persist across part boundaries. Focused regressions place
    invalid candidates both before and after the eventual exact boundary while
    preserving every later valid message and accepted folder. Complete
    core/writer suites, strict Clippy, and repeated combined-manifest full gate
    `.agent/test-results/1784490091-full` pass after remediation.
    A second fresh review found that rejected catalog folders could still be
    synthesized from candidate headers and that an all-terminal batch tried to
    project an empty writer. Catalog keys now prevent rejected source folders
    from re-entry, newly recovered header-only folders pass the same independent
    layout validator, writer `begin` enforces layout validation itself, and an
    empty transaction completes partial without projection. Regressions cover
    an overlong candidate-owned folder plus all-terminal input. Complete suites
    and strict Clippy pass after this remediation. Repeated combined-manifest
    full gate `.agent/test-results/1784490465-full` also passes.
    A third review found that a folder rejected only for source container-class
    metadata could not use a valid candidate-derived class, and that the product
    spec prematurely advertised the still-pending direct/restartable mode.
    Candidate-derived folder metadata is now admitted only after independent
    layout validation while the rejected source metadata remains counted;
    overlong paths remain contained. The product spec again documents the
    implemented durable-spool CLI, while direct mode remains explicitly pending
    in Checkpoint 5. Focused tests and repeated combined-manifest full gate
    `.agent/test-results/1784490920-full` pass. ScanPST-first and Outlook human
    acceptance remain.
    A fourth fresh review found three milestone-relevant containment/resume
    defects: a failed candidate-derived folder class reserved the folder key
    before a later valid candidate could recover it; named-property discovery
    excluded terminal durable candidates and could therefore renumber later
    properties after resume; and the metadata header pass loaded ownership for
    every candidate. Folder keys are now reserved only after successful
    independent validation, while failed attempts are counted once and remain
    retryable. The source-wide named-property identity query includes spooled,
    written, unsupported, and failed durable candidates. Top-level headers and
    their ownership are read together in bounded 1,024-row pages and reduced
    immediately to the compact packing index. Regressions cover invalid-then-
    valid candidate folder metadata, terminal-status catalog stability, a
    1,025-row page boundary, exclusion of embedded candidates, and the 64 MiB
    versus 4 GiB adaptive projection policy. The canonical combined-manifest
    full gate passes at `.agent/test-results/1784491725-full`.
    The next focused review found that an exact public part or sidecar
    filename conflict was detected only after writing, validating, and hashing
    the private PST. The intended names are now checked before transactional
    writer construction, while publication retains its no-clobber race check.
    A regression proves a conflicting `part-0001.pst` creates no transaction
    scratch. The repeated canonical full gate passes at
    `.agent/test-results/1784492088-full`.
    A fresh clean-context final review returned `CLEAN`: no blocker, high, or
    outcome-relevant medium findings remain.
    Human ScanPST rejected r3 parts 0001, 0003, 0004, and 0005. Every failure
    reported FAI Associated Contents rows that disagreed with their message
    subobjects; part 0001 also reported shared BID `0x34` at `cRef` 147 versus
    146. The normative FAI template marks `PidTagDisplayName` as copied from
    the message PC. PSTForge had synthesized that value only in the table row
    when the source property was absent, or synthesized it alongside a
    separately streamed source value. Associated `0x3001` is now materialized
    and normalized once, then emitted identically in the PC and row. The
    shared empty FAI-table BID now counts Deleted Items only when Deleted Items
    actually references that shared block. Focused PC/table, private-Deleted-
    Items refcount, independent libpff roundtrip, complete writer/core, and
    private external-corpus tests pass. The canonical full gate passes at
    `.agent/test-results/1784496402-full`.
    The focused fix review found that duplicate source `0x3001` records bypassed
    normal first-value deduplication and that an explicit empty Unicode value
    was incorrectly marked partial before safe normalization. Associated
    display-name translation now applies the standard duplicate containment,
    preserves a readable empty string for derived/generated normalization, and
    omits malformed values with explicit accounting. A production-path
    regression covers empty-first plus duplicate-later input. The repeated
    canonical full gate passes at `.agent/test-results/1784496760-full`.
    The next focused review found that completed-store validation compared the
    associated-contents row display name with its expected value but did not
    compare the message property context's `PidTagDisplayName` with that same
    value. Publication validation now opens associated message PCs with
    `0x3001` and requires exact PC/table/expected equality. The focused writer
    regression covers both source-provided and derived display names. The
    canonical combined-manifest full gate passes at
    `.agent/test-results/1784496965-full`.
    A fresh pre-publication review found no production defect. Its only
    finding was a hypothetical external-corpus comparator mismatch for an
    empty-first/duplicate-later associated `0x3001`; the production
    translation path already has a focused regression for that malformed
    input, so this test-harness-only extension does not block the real-spool
    qualification.
    Qualification r4 was republished from a copy-on-write clone of r3's
    retained 10.2 GiB payload pack without restarting libpff. The five parts
    completed in 2:45.09 at 247,988 KiB maximum RSS, with one serialization
    each, 37,035 written candidates, 19,333,198,848 total output bytes, and an
    unchanged source. Part lengths and per-part item counts are identical to
    r3. The only byte-identical part is part 0002, which was also the only r3
    part that passed ScanPST; parts 0001 and 0003-0005 changed under the FAI
    row/PC and shared-table refcount corrections. ScanPST-first human
    acceptance of r4 remains pending.
    A debug-only replay of the 367 durable unsupported candidates in a
    temporary clone identified 316 writer-managed `PidTagBody`/raw-property
    collisions (285 contacts and 31 mail messages), 42 recipient-table heap
    page limits, five general heap page limits, two raw-property
    representation failures, and one aggregate-recipient-metadata failure.
    One additional embedded mail item is stranded beneath a rejected parent.
    These are whole native items absent from output, distinct from bounded
    property omissions, and remain a data-correctness gap. The temporary clone
    was removed after aggregate evidence was retained under untracked
    `.agent/test-results/v043-unsupported-diagnostic/`.
    The owner reports all five r4 parts clean in ScanPST and operational in
    Outlook. This closes the 0.4.3 performance milestone; the 367 unsupported
    native candidates become the first blocking evidence for the next
    data-correctness patch milestone.
  - [ ] Checkpoint 5: make bounded non-restartable streaming the default
    `split` execution mode. Add explicit `--restartable` durable
    ledger/payload-spool selection; restrict `--resume` and `--keep-work` to
    that mode; report estimated and measured temporary writes and peak output
    allocation. Prove that streaming does not create a mailbox-sized payload
    pack, handles an oversize single item without whole-item RAM
    materialization, preserves finalized parts on interruption, and stays
    bounded on the 19 GB qualification before the 83 GB release-scale source.
    This checkpoint is explicitly deferred by owner direction at 0.4.3 close.
    The accepted incremental restartable path meets the immediate performance
    need; direct-mode write-amplification work must not delay the newly exposed
    native-item data-correctness remediation.
- [ ] Milestone 0.4.4: Whole-Job Data Reconciliation.
  - [x] (2026-07-19) Created
    `milestone/v0.4.4-data-correctness` from merged and pushed `main` commit
    `aefcbb7`. The 0.4.3 qualification establishes 37,402 readable native
    candidates, 37,035 written candidates, and 367 whole-item omissions.
  - [x] (2026-07-19) Checkpoint 1: preserve explicit empty writer-managed body properties
    instead of rejecting the complete item. Recover the 315 complete and one
    partial candidates whose Unicode `PidTagBody` is readable and explicitly
    empty. Add normative conformance traceability, focused translation/writer
    tests, independent-reader validation, and ScanPST-first evidence.
    Microsoft MS-OXCMSG 2.2.1.58.1 defines `PidTagBody` as `PtypString`
    without a non-empty restriction. Production translation now maps the
    exact observed zero-byte Unicode property to a present empty body instead
    of a conflicting raw property. The writer encodes the empty variable
    value with a null HNID while retaining the property entry, and completed
    validation distinguishes that present-null value from property absence.
    Focused production-translation and writer reopen tests pass; complete
    core and writer suites pass with 75 and 87 tests respectively. Fresh
    adversarial review and ScanPST-first acceptance remain.
    The first clean-context review found one high and two medium gaps:
    embedded validation did not accept the same present-null empty value,
    native-body containment ignored an in-memory empty plain body, and the
    production regression did not serialize the folder-store path. Embedded
    and normal completed-store validation now distinguish present empty from
    absent, plain native-body presence includes the in-memory representation,
    and the production regression writes and validates a real mail store.
    Existing streamed bodies continue through their independent
    type/length/hash validator. Focused tests and the repeated complete
    core/writer suites pass after remediation. A fresh final-state
    clean-context review returned `CLEAN`. The external r1 checkpoint is
    271,360 bytes with SHA-256
    `1b0fbbceee053302cba10d7aa17e1fa7d9955047e6610c5dcf47a12a1fef7d19`;
    `pffinfo` identifies a 64-bit PST and `readpst` extracts exactly one
    message with the expected subject and empty body without diagnostics.
    Human acceptance confirms ScanPST and Outlook both pass: the unrepaired
    candidate opens normally, contains the one expected message in the
    expected folder, and preserves its explicit blank body. The canonical
    full gate passes at `.agent/test-results/1784499956-full`.
  - [x] (2026-07-19) Checkpoint 2: replace the recipient-table single-heap-page limitation
    with specification-conforming scalable storage. Recover 42 candidates,
    including eight otherwise complete items, and prove recipient count,
    roles, addresses, and property rows through independent reads.
    The writer now retains its compact single-page TC when it fits and falls
    back to the existing specification-backed multi-page HN/BTH, data-tree row
    matrix, and subnode-value representation otherwise. Focused cases cover
    long variable values, 448 rows crossing the 447-record BTH leaf capacity,
    a later transactional message, part-boundary rollback/reappend, and the
    same external TC in an embedded message. Completed-output validation now
    compares every recipient row for normal, associated, and embedded
    messages. Two clean-context reviews identified and closed the later-message
    ownership and validation gaps; the final fresh review returned `CLEAN`.
    The writer suite passes 88 active tests with one existing ignored scale
    test, and the fast gate passes at
    `.agent/test-results/1784500826-fast`. External candidate r1 is 779,264
    bytes with SHA-256
    `b34c927f41e074028d4a3487a21de0c49b1088cf9141c1e013c0beb2d89f0fc9`;
    `pffinfo` accepts it and `readpst` extracts exactly the two expected
    top-level messages without diagnostics. ScanPST-first and Outlook
    acceptance remain. Human ScanPST r1 instead found an exact size-accounting
    failure: the 448-row external TC's 43,256 bytes of XBLOCK/SLBLOCK/SIBLOCK
    index payload were added to both embedded and containing Message object
    sizes. The repaired reference retains the data and changes the embedded
    message from 112,998 to 69,742 bytes and the containing message from
    226,804 to 140,204 bytes. EMP-16 records the resulting logical-data size
    rule; r1 is rejected. The first remediation review found the same
    structural-index overcount in streamed XBLOCK/XXBLOCK paths. Those paths
    now use the same data-only contribution, with a regression proving equal
    top-level, attachment, and embedded sizes for identical in-memory and
    multi-block spooled properties. The complete writer suite passes 89 active
    tests with one existing ignored scale test; the fast gate passes at
    `.agent/test-results/1784503858-fast`, and a fresh final review returned
    `CLEAN`. Replacement r2 is 779,264 bytes with SHA-256
    `773be042e192595b69e35309fea059365bdcdd75ea5e84fb64743a849576b2ec`.
    Its failed object chain exactly matches the repaired r1 values: containing
    message 140,204, attachment 69,862, and embedded message 69,742 bytes.
    `pffinfo` and `readpst` accept r2 and extract both top-level messages
    without diagnostics. Human acceptance confirms r2 is clean in ScanPST and
    passes the required Outlook message, recipient-list, and embedded-message
    checks. The canonical full gate passes at
    `.agent/test-results/1784504284-full`.
  - [x] (2026-07-19) Checkpoint 3: remove the general message heap-page limitation for the
    five affected candidates without imposing an arbitrary source-property or
    item-count cap. Validate the exact resulting property contexts and tables.
    The implementation now retains the compact single-page PC when it fits and
    otherwise packs bounded values into HN continuation-page allocations,
    builds the documented 2-byte-key/6-byte-value PC BTH across leaf and
    intermediate levels, fills non-final HN pages, updates root/bitmap fill
    levels, and points the owning message or attachment node at the resulting
    data-tree root. Heap data trees now extend through XXBLOCKs rather than
    stopping at the former 1,021-page helper limit. The same builder owns
    top-level, associated, embedded, and attachment PCs. Focused regressions
    cross both the single-page heap boundary and the 447-record BTH-leaf
    boundary in top-level and embedded messages, then prove transactional
    rejection rollback/reappend is byte-identical to direct output. The
    writer suite passes 90 active tests with one existing ignored scale test;
    the fast gate passes at `.agent/test-results/1784504986-fast`. Fresh
    adversarial review found one high boundary defect: 2,048 accepted
    non-empty variable properties could exhaust page-zero HID indexes before
    the compact serializer reached its page-size fallback. Compact HID
    exhaustion now takes the same scalable path, and a full-store regression
    writes and reopens the maximum currently accepted variable-property
    collection. A fresh final review, repeated gate, and external
    ScanPST-first acceptance remain. The repeated fast gate passes at
    `.agent/test-results/1784505308-fast`.
    The fresh final-state review returned `CLEAN`; it found no remaining
    blocker, high, or medium issue. The combined external r5 candidate is
    271,360 bytes with SHA-256
    `a01e42a39063c63fa3dd982efb47e167f7530eb6e5eb6074ea6b8a10693f85cf`.
    Its first top-level message and embedded child each force HN continuation
    pages with ten 1 KiB values. Its second top-level message has 505 custom
    properties and forces a level-1 PC BTH. `pffinfo` 20231205 accepts the
    complete file. `readpst` 0.6.76 extracts the first message without
    diagnostics, then its deep parser rejects the second message solely
    because the BTH header has the documented `cbKey=2`, `cbEnt=6`,
    `bIdxLevels=1` form; the same reader extracts the identical 500-property
    message while it remains a compact single-page PC. This isolates an
    independent-reader limitation rather than a generic property-count
    failure. The deliberately larger diagnostics are retained only under
    ignored `.agent/test-results/v044-general-pc-diagnostics/`; r5 is the sole
    human acceptance directory. Human acceptance confirms ScanPST is clean
    and Outlook opens both r5 messages and the embedded child with the expected
    content. The project owner accepts the isolated `readpst` limitation as
    non-blocking for this documented structure. The canonical full gate passes
    at `.agent/test-results/1784506612-full`.
  - [x] Checkpoint 4: diagnose from durable local evidence, then resolve the
    two raw-property representation failures and one aggregate-recipient
    metadata failure. A retained-spool replay proved the previously stranded
    embedded item is recovered naturally when its parent becomes writable:
    363 accepted top-level candidates produced 364 messages. No independent
    child-promotion behavior is required. The remaining failures are three
    ordinary top-level messages: two named binary values of 19,811 and 64,051
    bytes rejected by the obsolete 16 KiB raw-value ceiling, and one
    368-recipient message with 31,140 bytes of aggregate display/address text
    rejected by the obsolete 16 KiB aggregate ceiling. Both limits predate the
    documented scalable PC/TC implementations and are not PST format
    boundaries. Remove only those artificial gates, retain the existing
    per-value, collection-count, and 32-bit PST size bounds, and add focused
    regressions. Do not use an adversarial reviewer for diagnosis when the
    cause is locally clear.
    Implementation removed the obsolete single-page limits from PC raw-value
    subnode externalization and aggregate recipient/custom-property
    validation. The real 368-recipient case also exposed a masked writer
    defect: a singleton folder contents table always selected the compact TC
    encoder even when `PidTagDisplayTo` required an external value. Singleton
    normal and associated contents now fall back to the documented scalable
    TC representation on compact HN exhaustion. The complete writer suite
    passes with 90 active tests and one intentionally ignored large-boundary
    test.
    A current-binary replay of only the three remaining candidates from the
    retained 19 GB spool published
    `qualification-v044-replay-r1/parts/part-0007.pst` in one serialization:
    779,264 bytes, SHA-256
    `8bcb4fd0c7516e8eb72b5e7e1db42c9e94faf0a281aefc42519b8aa634f7bc4e`,
    three top-level messages, and one source folder. The durable ledger now
    contains exactly 37,402 `written` candidates, no other candidate status,
    zero `output_unrepresentable` events, and passes SQLite integrity check.
    `pffinfo` 20231205 accepts the part; `readpst` 0.6.76 extracts all three
    messages with zero skipped. Fresh adversarial review, fast/full gates, and
    ScanPST-first/Outlook human acceptance remain pending.
    The first clean-context review found one newly reachable high issue and
    one medium resource issue. Removing the aggregate limit allowed 339
    external raw values plus the two mandatory table subnodes to exceed a
    message's 340-entry SLBLOCK, while the top-level message builder still
    emitted only a single leaf. Message subnode roots now preserve their
    assigned BID while selecting a leaf through 340 entries or a documented
    level-1 intermediate tree above it; focused 338/339 external-property
    tests pass. The review also correctly rejected `i32::MAX` as an
    operational in-memory acceptance limit because current serialization can
    hold approximately four copies of materialized bytes. Per-value
    materialization is now capped at the core translator's existing 1 MiB
    bound, and aggregate in-memory custom properties are capped at 128 MiB,
    bounding that peak near 512 MiB before fixed overhead and retaining
    headroom beneath the 2 GiB gate. Source properties already classified as
    stream-capable continue to use the spooled path; this change does not claim
    that every named-property type is stream-capable. A fresh final-state
    review remains required.
    The next fresh review found that the prescribed-root builder had only
    replaced top-level message SLBLOCK emission; recursively embedded messages
    still emitted one leaf. It also found that the 128 MiB budget excluded
    attachment raw properties and embedded descendants. Both findings were
    high and applicable. Embedded messages now use the same scalable
    prescribed-root builder, with focused top-level and embedded 338/339
    external-property boundaries. One checked budget now accumulates every
    materialized message, attachment, and descendant custom-property payload
    in the top-level item graph. Arithmetic boundary coverage accepts exactly
    128 MiB and rejects the next byte without allocating a boundary-sized
    fixture. The focused regression passes; another fresh final-state review
    remains required.
    The following fresh review confirmed scalable root construction and block
    identity/refcount behavior, then found two medium enforcement/coverage
    gaps: public transactional append bypassed the new resource validation,
    and tests did not directly exercise transactional rollback/reappend,
    singleton associated external TC fallback, or graph-wide aggregate
    traversal. Transactional append now validates aggregate and message
    contracts before mutating state. Focused tests cover transactional
    top-level and embedded 341-entry roots through rollback/reappend, a
    singleton associated row with a 4,000-byte display value, and a 3+4+5-byte
    top-level/attachment/embedded graph accepted at 12 bytes and rejected at
    11. A fresh final-state review remains required.
    The final clean-context gate review returned `CLEAN`. It verified public
    one-shot and transactional validation, top-level and embedded level-1
    subnode roots, prescribed BID/refcount handling, rollback/reappend
    coverage, graph-wide resource accounting, singleton associated external
    fallback, and preservation of the three real-candidate recovery changes.
    The canonical combined-manifest full gate passes at
    `.agent/test-results/1784511461-full`, including licenses, advisories,
    writer `pffinfo`/`readpst`, writer acceptance, and all external-corpus
    reader pairs. ScanPST-first and Outlook human acceptance of retained-spool
    part 7 passed. One message displays corrupted quoted text, but the owner
    confirmed the same corruption in the 19 GB source. Its source
    `PidTagBody/PtypString` and 64,051-byte `Internet Charset Body` named
    binary contain the same damaged text, code page 65001 is present, and no
    HTML or RTF alternative exists. Preserving those source values exactly is
    the correct recovery outcome; no output reconstruction is justified.
  - [x] Checkpoint 5: persist a bounded structured rejection category and
    safe diagnostic summary whenever a candidate cannot be written. The
    ledger event must not be `{}` and must never include subjects, addresses,
    bodies, filenames, payload bytes, or private paths. `recovery.log` reports
    exact aggregate counts without itemizing private mailbox data.
    The durable category is a closed enum rather than captured error text:
    source item reported unsupported, malformed candidate, malformed property,
    writer input rejection, item graph dependency rejection, unsupported
    embedded item, or embedded item stranded beneath a finalized parent. The
    split JSON and bounded human log expose exact aggregates from those durable
    events. Unknown, malformed, missing, duplicate, or status-contradictory
    rejection events are ledger-integrity failures; writer append failures
    remain job failures and are not recast as safely contained omissions.
    The implementation persists versioned enum-only metadata, attributes a
    direct translation failure only to its identified item, attributes
    ancestors to item-graph dependency, and leaves descendants and siblings
    for the recursive stranded-parent pass. Aggregate counts are emitted in
    split JSON and the bounded privacy-safe human log. Read-only resume
    validation now enforces foreign keys plus the bidirectional rejection
    event/status invariant before publication reconciliation, spool cleanup,
    or any other application mutation. Regression coverage includes durable
    reopen, malformed metadata, contradictory status, an orphan rejection
    event, unchanged ledger/payload/output state after refusal, and nested
    direct/dependent attribution. The final clean-context review returned
    `CLEAN`; the canonical combined-manifest full gate passes at
    `.agent/test-results/1784515992-full`.
  - [x] Final gate: reconcile the 19 GB source's 37,402 readable candidate
    keys to exactly 37,402 unique written item keys across finalized parts,
    with zero unexplained `unsupported`, `failed`, stranded, duplicated, or
    unassigned candidates. A writer limitation is a defect, not an approved
    omission. Any claimed impossible recovery requires specific source-read
    or Unicode-PST representation evidence and explicit human approval.
    Require the canonical full gate, independent `pffinfo`/`readpst`, ScanPST
    on every part, Outlook item/folder checks, unchanged source identity, one
    serialization per normal part, less than 2 GiB RSS, and the accepted
    one-minute-per-source-GiB cold-run ceiling.
    Final qualification `qualification-v044-final-r1` completed in 9:30.47
    with 323,200 KiB maximum RSS and five one-serialization parts totaling
    19,478,205,440 bytes. Its ledger has exactly 37,402 unique `written`
    candidates and 37,402 unique part assignments, with zero unsupported,
    failed, stranded, duplicate, or unassigned candidate. SQLite integrity and
    foreign keys are clean; all five parts passed supervised `pffinfo` and
    `readpst` before atomic publication; their hashes reverified; and source
    identity plus SHA-256 remained unchanged. Three omitted attachments map
    exactly to three source `attachment_missing` events rather than readable
    payload loss. The owner accepted the existing ScanPST/Outlook evidence and
    this final current-code reconciliation as completion of version 0.4.4.
- [x] Milestone 0.4.5: Direct-Write Performance.
  - [x] Started `milestone/v0.4.5-direct-write` in sibling worktree
    `../pstforge-worktrees/v0.4.5-direct-write` from approved and pushed
    `main` commit `036fb53`.
  - [x] Checkpoint 1: established the versioned execution-mode contract. Bumped
    every producer/package version to 0.4.5, added `--restartable`, rejected
    `--resume` and `--keep-work` without it before creating output, persisted
    the selected mode in job identity and reports, and retained compatibility
    for explicitly restartable pre-0.4.5 jobs. The fast gate passed at
    `.agent/test-results/1784520407-fast`; focused tests prove read-only direct
    refusal, restartable-only option refusal, persisted mode mismatch refusal,
    legacy missing-mode interpretation, and an actual 0.4.4 ledger reopen.
    Clean-context review found and resolved the skipped 0.4.4 schema fallback;
    final clean-context review reported no blocker or high finding.
  - [x] Checkpoint 2: added documented writer primitives for a preflighted
    streamed external value. Derive physical allocation from declared property
    and attachment lengths plus PST block/table framing before consuming
    payload bytes; stream chunks directly into the active message transaction;
    hash and length-check at end; and roll back the exact message transaction
    on abort. Add boundary, empty, huge, interrupted, malformed-length, and
    rollback/reappend writer tests plus conformance references. Direct
    projection now runs the writer's real block-allocation and final-index
    calculation without opening a payload stream or modifying the private
    file. The subsequent append requires an identical projected final EOF.
    Direct hashes are returned as one message-atomic completion result only
    after the actual finalized EOF matches the preflight token; the source
    interface has no completion callback that could release data before
    acceptance. Aggregate message-size bounds are checked before projection or
    opening any direct stream. Any read, length, hash, projection, or
    interruption failure restores the exact private checkpoint. Focused tests
    cover XBLOCK/XXBLOCK boundaries, direct OLE, message and attachment
    properties, empty and
    over-limit declarations, aggregate over-limit messages, short/long streams,
    bad hashes, mid-stream interruption, exact and mismatched projections, and
    rollback/reappend. The complete writer suite passed 96 tests with its
    existing multi-gigabyte test ignored. The
    first fast gate exposed and corrected an adjacent-range spool regression.
    A clean-context review then found premature per-blob completion and a
    missing aggregate direct-message bound; both were corrected as described
    above. A second clean-context review found that the bound omitted nested
    attachment payloads and that native-body checks recognized only spooled
    body properties. Recursive attachment accounting and shared
    spooled/direct plain-text, HTML, RTF, and RTF-sync handling now cover top
    and embedded messages, with zero-open nested-overflow and direct-body
    regressions. A third clean-context review found direct/spooled parity gaps
    for inline empty binary OLE and recursive completed-store identity checks.
    Direct mode now emits empty binary OLE inline without opening a stream at
    both top and embedded levels, and recursive validation verifies direct
    property type, length, and hash. A negative embedded-identity regression
    proves a mismatch blocks validation. A fourth clean-context review found
    that an inline empty direct OLE descriptor could declare a contradictory
    hash; preflight now requires an optional hash to equal SHA-256 of empty
    content, with top-level and embedded negative regressions. The post-fix
    state was reviewed again: projection rollback still issued a same-length
    `ftruncate`, and completed identity validation was not recursive for every
    normal and associated message. Projection now restores only in-memory
    state and a metadata regression proves no file timestamp, allocation, or
    length change. One recursive validator now covers every accepted message;
    negative tests cover a first associated direct attachment and a later
    message's nested direct attachment. The complete writer suite passes 97
    tests with its existing multi-gigabyte test ignored. The post-fix fast gate
    passed at `.agent/test-results/1784523711-fast`.
  - [x] Checkpoint 3: implement the direct supervisor/sink. Perform bounded
    per-item structural preflight, select the destination part before payload
    streaming, translate parser events without a payload pack, and keep only
    compact accounting needed for reports and exact reconciliation. Permit
    arbitrary source traversal while producing deterministic folder tables
    and identifiers. A boundary decision must not require rereading or
    rewriting a completed payload.
    The completed direct supervisor uses one contained parser traversal. For
    each top-level message graph it captures only bounded property and
    attachment prefixes, declared lengths, and structural metadata, chooses
    the destination part, then streams every unread payload remainder from the
    still-open native handle exactly once. Parent metadata is committed before
    buffered embedded descendants so the transactional ledger remains flat;
    the payload phase retains native writer order. Memory is bounded to one
    message graph and its configured prefixes rather than the mailbox.
    Restartable mode retains the existing durable full-payload protocol.
    - [x] Added a distinct non-resumable metadata capture mode to the existing
      private SQLite catalog. It stores at most 64 KiB per property and 16 KiB
      per attachment directly in SQLite, assigns stable direct-stream IDs to
      uncaptured remainders, preserves declared logical lengths through the
      canonical model, and leaves `payload.pack` at zero bytes. Canonical
      translation now emits direct message properties, attachment properties,
      binary attachments, and OLE attachments while continuing to decode
      bounded scalar metadata and MIME signatures from captured prefixes.
      Prefixes are never promoted to complete named or mapped scalar values:
      values without a direct writer representation are explicitly counted as
      omitted instead of silently truncated. Worker retry reopen restores the
      bounded capture policy and continues direct-stream IDs above the durable
      maximum.
      Focused tests cover transactional inline capture, logical-size
      accounting, direct descriptor identity, named-property containment,
      retry continuity, and zero payload-pack growth.
    - [x] Added the bounded second-pass protocol reader. It reconstructs the
      same durable message identities as the metadata pass, skips payload
      streams the writer did not select, exposes a requested stream as a
      bounded `Read`, and stops exactly at the next protocol control frame.
      The transactional writer no longer assumes messages arrive grouped by
      folder: normal and associated contents rows retain their explicit parent
      folder node and finalization groups them by that relationship. Completed
      validation obtains message node IDs from the emitted table rows rather
      than predicting IDs from folder iteration. Focused regressions cover
      omitted-stream skipping and interleaved `A, B, A` source traversal with
      normal and associated messages; the complete writer and core suites pass
      (98 writer tests plus one ignored multi-gigabyte case, and 82 core
      tests).
    - [x] Connected the default direct path for both balanced and aggressive
      PST output. The bounded metadata pass fixes the folder and named-property
      catalogs; the supervised writer-order pass registers each selected
      direct stream, projects exact final EOF before opening it, streams it
      once into the active transactional PST, and publishes through the
      existing independent-validation and atomic-ledger path. Direct output
      refreshes the worker watchdog during payload and discard chunks, refuses
      a requested stream that is absent before the current top-level message
      ends, requires protocol EOF, and rechecks filesystem capacity against
      exact incremental PST growth. Fully captured metadata remains inline;
      replay-required properties and attachments use direct IDs without
      changing native-body containment. Real external document, named
      property, OLE, and reference-attachment cases are byte-identical to
      restartable output and have complete accounting. The public Enron and
      focused PIM fixtures cannot serve as boundary evidence because both the
      accepted restartable path and direct path hit the same pre-existing deep
      embedded validation failures before publication; no milestone conclusion
      is drawn from those cases. The complete job/core/writer suites pass, and
      the fast gate passed at `.agent/test-results/1784528600-fast`.
    - [x] Closed the direct-stream supervision review findings. The full-payload
      pass now binds every embedded message through its durable parent item key
      and attachment index, while top-level duplicates retain their cataloged
      occurrence; it no longer reconstructs embedded keys from traversal
      order. The watchdog remains active until the child is reaped, so a worker
      that emits completion and then hangs is still killed. Direct parsing gets
      at most three clean attempts: later attempts drain already-written and
      already-unsupported top-level candidates, rebuild only the unpublished
      active part, and retain atomically published parts. Exhaustion is a typed
      `failed-partial` terminal result with a retained compact ledger and
      recovery log, not an opaque error or a resumable state. A one-time
      injected abort on the external document case recovered on the second
      metadata pass and second direct pass, wrote its sole candidate exactly
      once, and completed without omissions. A direct-only abort on every
      attempt stopped after three failures with zero published candidates,
      exit status 1, unchanged source identity, and
      `Terminal failure: worker_protocol` in `recovery.log`. A follow-up
      clean-context review found that raw pipe EOF inside an opened payload
      escaped as an ordinary writer I/O error and that all-candidate identity
      maps were retained unnecessarily. Worker payload I/O now carries a typed
      marker into the retry classifier. An injected abort one byte into a real
      OLE payload failed the first full-payload pass, rebuilt the unpublished
      part, and completed on the second pass with one exact candidate, no
      omissions, and no terminal failure. Top-level identity is resolved
      incrementally from an indexed, scalar SQLite row cursor; ledger terminal
      status is queried by durable key, and embedded/message stream bindings
      are registered only for the current top-level tree and cleared when it
      drains. No all-candidate identity, terminal-key, or occurrence map remains
      in supervisor RAM. A staged-file cleanup guard removes the unpublished
      active PST on retry, exhaustion, interruption, or any other unwind;
      every-attempt failure evidence retains only compact ledger state and a
      zero-byte payload pack. Ledger interruption during publication now
      reconciles the intent and rename state before building the terminal
      interrupted snapshot, so an atomically published part cannot be omitted
      from the report or assigned twice.
      The affected suites pass (83 core, 53 job, 98 writer plus the intentional
      ignored multi-gigabyte case, and 8 CLI tests), and the canonical fast gate
      passed at `.agent/test-results/1784531283-fast`. A final fresh
      clean-context adversarial review found no blocker or high
      milestone-relevant issue.
    - [x] Corrected the first 19 GB direct qualification failure without
      retaining payload data. The metadata pass cataloged all readable
      candidates in about three minutes with roughly 260 MiB worker RSS and a
      zero-byte payload pack, but every full-payload attempt stopped because an
      unsupported embedded child was absent from the old spooled-only binding
      tree. Direct binding now traverses the durable catalog beneath the
      current top-level item for spooled, written, and unsupported descendants,
      and registers those bindings before a terminal root is drained on retry.
      A focused regression covers an unsupported child beneath an
      already-written root. The job and core suites pass (53 and 83 tests), the
      canonical fast gate passed at
      `.agent/test-results/1784532127-fast`, and a fresh clean-context review
      found no blocker or high milestone-relevant issue.
    - [x] Identified and contained process-unstable libpff identifiers for
      embedded messages. The same child beneath source parent node 3283844,
      attachment zero, appeared as node 3408225810 during the r3 metadata pass
      and node 3283876 during every r3 full-payload pass; r2 metadata reported
      node 3075377693. Direct replay now matches embedded messages by the
      durable parent item key, attachment index, provenance, and recovery
      index, while top-level messages continue to require their stable source
      node identity. A regression deliberately routes worker child ID 20 to
      durable child identities 99 and 100 under distinct parents and verifies
      both payloads. Reduced-key collisions remain terminal. The complete core
      suite passes and a fresh clean-context rereview found no blocker or high
      issue.
    - [x] Aligned damaged embedded-item acquisition across the two direct
      passes without violating the metadata ledger's flat event contract. The
      r4 writer passed the former identity failure, wrote a 656 MiB active PST,
      then found a nested child that the source-order metadata pass had omitted
      after attachment-property decoding left its embedded parent partial.
      Direct metadata now uses a third `EmbeddedFirst` traversal policy: obtain
      and queue the child before decoding attachment properties, close the
      parent attachment and message, then emit the queued child. The full
      writer pass remains recursive and restartable recovery retains source
      order. Readable attachment properties are still attempted exactly once
      before every contained attachment abort. The libpff and core suites pass,
      the canonical fast gate passed at
      `.agent/test-results/1784534926-fast`, and final clean-context review
      found no blocker or high issue.
    - [x] Retained each queued embedded item's owning native attachment handle
      until that child finishes. r5 proved that a queued child header could
      remain readable after its parent closed while later native attachment
      access failed; the recursive payload pass succeeded only while the
      container stayed alive. On r6 the metadata catalog increased from 37,400
      candidates and 311 embedded descendants to 37,413 and 324, cleared both
      earlier zero-binding writer failures, and reached the 19-minute
      performance ceiling with a 3.54 GiB active PST before controlled
      interruption. Peak aggregate RSS remained 323,600 KiB and the active PST
      cleanup left only the compact ledger and zero-byte payload pack.
      The pending traversal already retained one native child handle per queued
      embedded item; retaining its container doubles that bounded-by-source
      handle set but does not add payload retention or change asymptotic
      breadth. Worker supervision contains native allocation failure. Wider
      pending-handle budgeting remains measurable parser hardening, not a
      reason to discard the additional readable children recovered here.
    - [x] Removed quadratic final-size planning from the single-part direct
      path. Qualification r6 spent 19:07 producing only a 3.54 GiB active PST
      while one supervisor core remained saturated and the NVMe device was
      mostly idle. The direct loop had rebuilt the finalized tables and B-tree
      plan twice for every appended message, making final-size calculation
      O(n²) in the message count. When the requested part limit is at least the
      source file size, PSTForge now performs one metadata-only, whole-part
      allocation projection, rolls that projected state back without touching
      the temporary PST, preflights disk space once, streams every accepted
      payload once, and verifies one final projection before publication.
      Projection and actual writing use the same writer allocation path; any
      byte-length divergence blocks publication. If the complete projected PST
      does not fit, the existing exact incremental split path remains active.
      A two-message direct regression proves that projection opens no payload,
      changes no temporary-file metadata, rolls back to the initial private
      state, and predicts the exact finalized byte length. The writer and core
      suites pass (99 writer tests plus one ignored multi-gigabyte case, and
      83 core tests), and the canonical fast gate passed at
      `.agent/test-results/1784537731-fast`. A fresh clean-context adversarial
      review found no blocker or high milestone-relevant issue in hard-limit
      enforcement, projection/write parity, payload isolation, memory bounds,
      cleanup, retry behavior, disk preflight, interruption, or publication
      safety.
    - [x] Corrected the first whole-part projection failure without violating
      the PST subnode format. Qualification r7 completed metadata capture in
      3:45 at 340,112 KiB peak RSS with a zero-byte payload pack, then stopped
      before payload streaming because one contents table exceeded the
      documented 173,400-entry SLBLOCK/SIBLOCK capacity. MS-PST requires
      `SIBLOCK.cLevel` to be exactly 1 and each SIENTRY to address an SLBLOCK,
      so a deeper subnode tree is prohibited. The external TC writer had
      unnecessarily assigned a subnode to every nonempty variable cell.
      Values at or below the documented 3,580-byte HN allocation maximum now
      use HIDs in packed continuation pages; only larger values consume
      subnodes. TC BTH records use the same allocator, page packing reserves
      structural fill-map space, bitmap fill levels reflect each actual page,
      and message/attachment size properties follow the smaller exact
      representation. A 24,800-row regression serializes 173,600 small
      variable values that the old representation necessarily rejected. The
      complete writer suite passes 100 tests with one intentional
      multi-gigabyte test ignored, and the core suite passes 83 tests. The
      canonical fast gate passed at `.agent/test-results/1784538763-fast`; a
      fresh clean-context adversarial review found no blocker or high issue in
      HNID selection, HID page/allocation numbering, bitmap cadence, BTH
      references, large-value fallback, size chains, bounds, determinism, or
      test coverage. Independent ScanPST and Outlook acceptance remains
      pending.
    - [x] Prevented a known corrupt recovery tail from rewriting a complete
      direct part. In r8, whole-part projection selected an exact
      19,530,195,968-byte output at 3:37, and the first payload pass streamed
      that complete extent in about 2:51. Libpff then returned the same global
      `recover_items` error already contained by metadata recovery. The old
      protocol treated the post-catalog error as a failed attempt, deleted the
      complete unpublished PST, and had rewritten 5.39 GiB when the run was
      stopped at 7:48. Writer-order workers now emit a distinct parser-boundary
      frame after rechecking source identity. The supervisor accepts it only
      when its durable top-level cursor is exhausted; a remaining expected
      candidate, an active message, a metadata worker, or any other placement
      of the frame remains a protocol error. This preserves retry behavior for
      actual omissions while eliminating a full-output rewrite caused solely
      by the already-accounted corrupt tail. A focused regression proves both
      the exhausted-catalog acceptance and missing-candidate rejection. The
      core suite passes 84 tests and the writer suite passes 100 tests with one
      intentional multi-gigabyte test ignored. The canonical fast gate passed
      at `.agent/test-results/1784539812-fast`; a fresh clean-context review
      found no blocker or high issue and confirmed that active frames,
      malformed ownership, payload EOF, trailing data, worker/watchdog failure,
      nonzero exit, missing durable candidates, and source identity changes
      remain hard failures.
    - [x] Proved the direct single-file performance and memory targets, then
      corrected a completed-store validator catalog mismatch exposed by the
      full source. Qualification r9 completed one catalog pass, one exact
      19,530,195,968-byte projection, and one payload write in 6:50.59 at
      467,584 KiB peak RSS. Qualification r10 repeated the same work in
      6:57.26 at 467,784 KiB peak RSS. Both kept `payload.pack` at zero bytes,
      accepted the known parser boundary only after durable catalog
      exhaustion, and cleaned the unpublished PST when validation failed.
      The writer correctly used the complete source-wide NAMEID catalog,
      including identities whose damaged or unsupported values could not be
      serialized. The final streamed-identity validator incorrectly rebuilt a
      smaller catalog from written values, shifted later mapped property IDs,
      and reported an existing Boolean property as absent. Final validation
      now receives the writer's authoritative catalog instead. A focused
      regression reserves an unused source identity before a written named
      property and proves transactional finalization retains the exact mapping.
      Privacy-safe mismatch diagnostics identify only the output message node,
      attachment index path, mapped property ID, and expected/actual MAPI
      types. The writer suite passes 101 tests with one intentional
      multi-gigabyte test ignored, and the core suite passes all 84 tests.
      Evidence is retained at
      `.agent/test-results/1784540039-v045-direct-single-r9` and
      `.agent/test-results/1784541051-v045-direct-single-r10`; the failed job
      directories contain no published PST and are removed after diagnosis.
    - [x] Removed runtime source/output digest passes and output-reader
      validation from the default direct path. Direct source identity now
      carries no SHA-256; supervised workers match stable descriptor/path
      metadata without hashing. Restartable source and part identities retain
      SHA-256. Direct manifests represent an uncalculated part digest as absent,
      and publication performs no content reread.
    - [x] Replaced the large-file reopen/rebuild with construction-time
      AMap/PMap/FMap/FPMap, DList, header free-map, free-count, and valid-status
      serialization. The writer performs one final file `fsync`; the job layer
      then uses same-filesystem atomic renames and directory `fsync` without
      rewriting the PST. The 129 MiB attachment regression crosses recurring
      FMap regions and passes the independent writer readers. The writer, core,
      and job suites pass, and the fast gate is retained at
      `.agent/test-results/1784552328-fast`.
    - [x] Added Linux source mutation protection before parsing: a read lease
      where supported, plus nonblocking whole-file OFD record and `flock`
      shared locks. A process-scoped POSIX record lock is used only when OFD
      locking is unavailable. `SIGIO` lease breaks feed the existing interrupt
      supervisor; conflicting locks refuse recovery, and unsupported leases
      retain both advisory protections plus final descriptor/path identity
      checks. Focused `flock` and record-lock conflict tests and the fast gate
      pass at `.agent/test-results/1784552889-fast`.
    - [x] The first post-change full corpus run exposed ordinary attachment
      payloads being streamed before their duplicate `PR_ATTACH_DATA_BIN`
      property in metadata/writer order. libpff shares stream state between
      those views, so the later property read returned zero bytes and falsely
      marked document and OLE candidates partial. Non-embedded attachment
      properties are again read before attachment payloads; embedded-message
      properties remain deferred only for the recursive writer ordering they
      require. Direct document and OLE fidelity cases now pass. Legacy
      determinism and interruption/resume cases explicitly select
      `--restartable`, matching the 0.4.5 CLI contract.
    - [x] The construction, publication, source-protection, optional-digest,
      dynamic NAMEID/folder, and attachment-order checkpoint passed the
      canonical full gate at `.agent/test-results/1784555196-full`, including
      all 26 external corpus cases and independent `pffinfo`/`readpst`
      acceptance. A fresh focused adversarial review reported no blocker or
      high-severity milestone-relevant findings.
    - [x] Collapsed metadata capture and payload replay into one libpff
      traversal. A top-level metadata/payload boundary retains native record
      and attachment handles only until that graph drains. Prefix bytes are
      consumed once and concatenated with the unread native remainder; neither
      source payload nor completed output is reread. Construction-time
      allocation maps, one final file `fsync`, and atomic no-clobber
      publication remain unchanged. Nested calendar-exception metadata is
      committed parent-first from a bounded graph buffer while native payloads
      retain recursive writer order. Late-discovered empty folders are
      observed before finalization. Parser-boundary accounting comes from the
      committed ledger if the optional completion frame is unavailable.
      The canonical full gate passed at
      `.agent/test-results/1784557059-full` after the calendar-exception
      regression exposed and verified the nested ordering correction.
      The final documented state passed the canonical full gate at
      `.agent/test-results/1784557519-full`. An injected direct worker abort
      after one complete candidate returned exit status 1 with one attempt,
      one failure, `worker_protocol`, zero written candidates, no published
      PST, a zero-byte payload pack, and only compact ledger/log state.
      The first clean-context review found one applicable high issue: bounded
      individual prefixes could aggregate without a graph-wide ceiling and
      were briefly retained in two representations. Direct metadata now
      charges serialized control bytes, twice the prefix length, and
      conservative per-frame overhead against a checked 256 MiB/262,144-frame
      per-graph budget before creating the second prefix copy. Exact-boundary,
      byte-overflow, and frame-overflow tests cover the limit; exhaustion is a
      recorded terminal partial result rather than supervisor OOM.
      The post-fix fast and canonical full gates pass at
      `.agent/test-results/1784557946-fast` and
      `.agent/test-results/1784557989-full`.
      A fresh clean-context adversarial rereview returned `CLEAN`: the prior
      aggregate-memory high finding is closed, and no blocker/high issue
      remains in FFI lifetime, one-pass fidelity, nested identity/order, hard
      limits, parser containment, cleanup, folder/NAMEID handling,
      publication/accounting, or source immutability.
    - [x] Completed the current-source single-part construction checkpoint.
      Qualification exposed that a libpff record entry borrows its record-set
      owner and that attachment data/property views share native cursor state.
      Record entries now retain their record set through the deferred payload
      phase; top-level and embedded message owners remain alive until that
      graph drains. Ordinary attachment data is streamed through the
      attachment API once, while duplicate `PR_ATTACH_DATA_BIN` property
      capture remains prefix-only. Reacquired attachment handles use the
      documented native seek operation to begin at the captured prefix.
      A source-size-plus-25-percent capacity check selects the single-part
      append path without per-message final-table projection; final byte size
      is still checked before publication. Final sidecar folder accounting
      uses the complete observed source-folder set because one-pass
      construction begins before later empty folders are discovered.
      An isolated full worker traversal completed in 180 seconds after the
      former native crash point. Qualification r10 then published one
      19,532,989,440-byte PST in 7:02.35 at 462,380 KiB peak RSS, with a
      zero-byte payload pack, one worker attempt, and unchanged source
      identity. The current source catalog contains 37,413 candidates and the
      output contains the same 37,413, with zero unsupported candidates, zero
      omitted attachments, zero omitted folders, and no unfinished items.
      The known post-catalog libpff recovery-tail error remains one contained
      issue. Automated acceptance is complete through the fast gate at
      `.agent/test-results/1784561084-fast` and the refreshed canonical full
      gate at `.agent/test-results/1784561714-full`, including every external
      corpus and independent `pffinfo`/`readpst` check. The documented diff
      then passed the fast gate at `.agent/test-results/1784561857-fast`; a
      fresh clean-context adversarial review returned `CLEAN`, with no
      milestone-relevant blocker or high finding. Human ScanPST/Outlook
      acceptance is the remaining checkpoint gate.
    - [x] Corrected the folder-identity defect exposed by the first human
      single-file gate. ScanPST found 149 folders and 37,060 items in r10, then
      reported two BBT reference-count mismatches, widespread swapped
      `PR_CONTENT_COUNT`/`PR_CONTENT_UNREAD` values, 38 orphaned messages, and
      hierarchy repairs. Transactional message rows had stored a folder node
      calculated from the currently known sorted path set; observing a
      lexicographically earlier folder later shifted existing node identities.
      Final folder properties used the complete sorted set, so rows, folder
      properties, shared empty-table selection, and BBT reference counts
      disagreed. Transactional folder nodes now follow append-stable first
      observation order, and final folder plans reuse those exact nodes.
      Construction additionally refuses any final folder whose expected normal
      or associated message count differs from the streamed rows bound to its
      node, preventing publication without a completed-file reread. A focused
      late-earlier-folder regression validates names, properties, contents
      rows, and messages through the independent writer reader; an injected
      shifted parent proves the construction-time refusal. The complete writer
      suite passes 104 tests with one intentional multi-gigabyte case ignored,
      and the fast gate passes at
      `.agent/test-results/1784562422-fast`.
      Qualification r11 published one 19,533,751,296-byte PST in 6:55.18 at
      462,420 KiB peak RSS. It again writes all 37,413 current-source
      candidates, reports 145 representable folders, zero omitted attachments
      or folders, one contained parser-tail issue, unchanged source identity,
      and a zero-byte payload pack. The refreshed canonical full gate passes
      every external corpus and independent `pffinfo`/`readpst` check at
      `.agent/test-results/1784562960-full`. A fresh clean-context adversarial
      review returned `CLEAN`, confirming stable uniqueness across folder
      locations, implicit parents, Deleted Items, empty-folder policies,
      rollback/projection, and independent parts, plus pre-BBT refusal of
      row/plan mismatch. The human owner then reported a clean ScanPST result
      and successful Outlook use of the r11 PST, completing the single-file
      direct-write acceptance gate.
  - [x] Checkpoint 4: complete direct publication and failure behavior. Keep
    one same-filesystem active PST temporary, construct every documented PST
    relationship and allocation structure before one final file `fsync`, then
    atomically rename it into `parts/` without a runtime reopen, output hash,
    or reader-validation pass. Preserve finalized parts on
    interruption, mark the direct job terminal partial, refuse resume, and
    require a new empty output directory for another attempt. Add disk
    exhaustion, worker crash, signal, output conflict, construction failure, and
    source-identity recheck tests. Construction-time allocation maps, final
    `fsync`, atomic no-replace publication, source locks and final identity
    checks, terminal direct failure accounting, partial cleanup, output
    conflict refusal, capacity refusal, worker abort/stall/protocol containment,
    and signal behavior are covered by the focused suites and canonical full
    gate recorded above. The accepted r11 proves the completed publication
    behavior through ScanPST and Outlook.
  - [x] Checkpoint 5: optimize the explicit restartable path without changing
    its recovery semantics. Buffer worker protocol control traffic and payload
    pack appends at durable candidate-batch boundaries, eliminate redundant
    metadata/seek syscalls from the append hot path, retain per-blob SHA-256
    and transactional truncation, and benchmark cold recovery plus retained
    replay. Treat the 0.4.4 9:30.47 cold run as the no-regression baseline.
    Worker protocol output now uses a 1 MiB buffer with an immediate hello
    flush and terminal flush, coalescing control and payload writes without
    weakening startup or completion supervision. The payload pack maintains a
    checked append cursor across each durable batch instead of issuing
    `seek(END)` and `metadata()` for every blob. Batch `sync_all` verifies the
    cursor against the filesystem before SQLite commit; SHA-256,
    deduplication, rollback truncation, resume reconciliation, and the
    128-candidate durability bound remain unchanged. Focused
    append/dedup/reopen tests and the fast gate at
    `.agent/test-results/1784564861-fast` pass. An initial clean-context review
    correctly found that generic buffering hid unit and durable-batch
    boundaries. Unit starts and each shared 128-candidate boundary now flush,
    and a buffered-protocol regression proves their immediate visibility. A
    second review questioned zero direct validator input, but that finding was
    challenged as inapplicable: direct calls `finalize_constructed`, whose
    false validation policy returns before either independent reader. A final
    fresh review confirmed the control flow and returned `CLEAN`. The 19 GB
    cold benchmark remains part of Checkpoint 7.
  - [x] Checkpoint 6: expose mode-specific telemetry for logical source bytes,
    payload-pack bytes, active-PST bytes, finalized bytes, validator reads,
    peak temporary allocation, peak RSS, elapsed time, and throughput. Keep
    user content out of logs and bound detailed recovery output. Reports now
    separate cumulative and peak payload-pack allocation, published active-PST
    serialization, finalized output, scheduled validator input, peak
    payload-plus-active allocation, RSS, elapsed time, and throughput. Linux
    `/proc/self/io` deltas provide measured supervisor physical reads and
    writes so logical workload is not presented as SSD traffic. Direct mode
    reports zero payload-pack and validator workload; restartable mode retains
    its logical validation workload. The fields contain no mailbox content.
  - [x] Checkpoint 7: run the canonical full gate and first qualify the 19 GB
    source as one default-direct PST by selecting a part limit above its
    recovered output size. Require exact current-source candidate assignment
    (37,413-to-37,413 for the 2026-07-20 source identity),
    unchanged descriptor/path identity, less than 2 GiB RSS, no more than one
    minute per source GiB, a clean ScanPST result, and Outlook content/folder
    acceptance before testing boundary packing. Then run the 4 GiB direct
    split regression and explicit `--restartable` mode; require identical
    aggregate results and independently valid outputs. Direct mode must show
    no mailbox-sized payload-pack allocation; restartable mode must improve or
    at least not regress the accepted 9:30.47 cold baseline.
    - [x] The accepted r11 single-file run completed the first half of this
      checkpoint as recorded above.
    - [x] The first current-code 4 GiB run
      `qualification-v045-one-pass-direct-4g-r1` wrote all 37,413 candidates
      once into five parts totaling 19,534,074,880 bytes, with zero unsupported
      candidates, omitted attachments, omitted folders, or payload-pack bytes.
      The four non-final parts were respectively 113,664, 2,653,184,
      1,383,424, and 113,664 bytes below 4 GiB. The final remainder was
      2,358,469,632 bytes. Source identity was unchanged and the known
      contained libpff recovery-tail issue remained the sole parser issue.
      This run is not accepted: it took 38:40.61, versus the 19-minute ceiling,
      and Linux reported 64,572,559,360 supervisor filesystem write bytes.
      Direct splitting rebuilt the complete finalization plan before and after
      every message, creating quadratic CPU work, while non-restartable bounded
      metadata incorrectly inherited restartable 128-candidate SQLite commits.
      Preflight now returns exact private and finalized extents; append verifies
      the private extent without rebuilding every final table, while finalization
      still independently enforces the exact finalized EOF before publication.
      Finalization groups streamed rows by parent once instead of rescanning
      every row for every folder. Direct metadata now commits at explicit
      worker/part/checkpoint boundaries; restartable mode retains the
      128-candidate durability bound. Focused extent-mismatch, folder-row,
      direct-boundary, and restartable durability tests pass, followed by the
      fast gate at `.agent/test-results/1784567988-fast`. Qualification r2
      was stopped after 12 minutes with two valid-size parts published because
      part 0002 alone still required about six minutes. Removing the second
      final-plan rebuild did not remove the first per-candidate rebuild, so the
      split remained above the one-minute-per-GiB ceiling. Its rejected output
      was deleted. Direct split projection now always computes the candidate's
      exact private allocation without opening payload streams, but constructs
      the complete final tables only in the proportional final
      `part_size / 16` boundary region. The append verifies the private extent;
      finalization independently recomputes the exact EOF and refuses any
      abnormal over-limit multi-message part before publication. Focused
      private-projection, direct-stream parity, and 64 MiB/4 GiB proportional
      boundary tests pass, followed by the fast gate at
      `.agent/test-results/1784569402-fast`. Qualification r3 must prove the
      runtime, exact boundary, and physical-write corrections before human
      ScanPST/Outlook work.
    - [x] Qualification r3 wrote the same five exact boundaries and all 37,413
      candidates in 9:21.78 at 328,712 KiB aggregate peak RSS. It reports zero
      unsupported candidates, omitted attachments, omitted folders, payload
      pack, or validator input; the sole issue is the known contained libpff
      recovery tail, and source identity is unchanged. This meets the
      one-minute-per-source-GiB target and reduces r1 runtime by 75.8%.
      Telemetry nevertheless measured 47,919,583,232 supervisor filesystem
      write bytes for 19,534,074,880 output bytes. The remaining amplification
      comes from SQLite spilling repeatedly modified direct-catalog pages
      during its intentionally long transaction. Bounded/direct capture now
      uses a fixed 512 MiB SQLite cache with dirty-page spilling still enabled.
      A first review correctly rejected disabling spill for an entire mailbox
      because memory would grow with the catalog. A proposed coarse automatic
      commit was also rejected: it could persist unpublished direct candidates
      and misalign a fresh one-pass worker retry. The fixed cache instead
      preserves the existing transaction and retry boundaries, covers roughly
      twice the observed per-part catalog growth, and bounds the SQLite cache
      well below the 2 GiB aggregate RSS ceiling. Explicit worker, part,
      signal, and completion checkpoints still commit, truncate the WAL, and
      sync the private directory. Restartable capture retains its default
      cache and 128-candidate durability boundary. Focused cache-mode and both
      commit-policy regressions pass. Qualification r4 then reproduced all five
      exact r3 boundaries and all 37,413-to-37,413 candidate assignments in
      8:37.05 with zero unsupported candidates, omitted attachments, omitted
      folders, payload pack, or validator input. The known contained parser
      tail remains the sole issue and source identity is unchanged. Supervisor
      peak RSS was 1,179,557,888 bytes, below the 2 GiB ceiling even with the
      worker, and measured supervisor filesystem writes fell to
      44,564,631,552 bytes. This cache-only reduction is modest; eliminating
      the remaining direct-ledger amplification requires a later catalog
      storage redesign rather than weakening transaction/retry correctness.
      All five r4 parts pass `pffinfo` and bounded one-part-at-a-time `readpst`
      extraction, with decoded scratch removed after every part. The canonical
      full gate passes at `.agent/test-results/1784571784-full`. Human
      ScanPST-first and Outlook aggregate acceptance then passed for all five
      r4 parts.
    - [x] Explicit restartable r1 wrote the same 37,413 durable item keys in
      five parts in 8:41.06, improving the accepted 9:30.47 baseline. It used
      an 11,013,437,088-byte peak payload pack and measured 120,416,735,232
      supervisor filesystem write bytes, versus direct r4's zero-byte pack and
      44,564,631,552 writes. Direct therefore saves substantial capacity and
      SSD traffic but only four seconds on this host. The restartable adaptive
      writer amortizes final-size projection, while direct still performs one
      exact private allocation simulation per candidate; a one-pass
      replay-free equivalent is the remaining split-performance opportunity.
      Ledger comparison found the exact same item-key set but 127 direct
      candidates marked partial that restartable marked complete. Every
      difference was a parent containing an embedded message, and both modes
      retained identical property, recipient, and attachment event counts for
      those candidates. Direct/writer-order recursion processed each child
      before the parent's `MessageEnd`, and the global issue delta incorrectly
      attributed child-local issues to the parent. Completeness now compares
      issues attributed to the current message ID; message-identifier failures
      and issue-log overflow remain conservatively partial. The libpff suite
      and fast gate pass at `.agent/test-results/1784574626-fast`. A bounded
      direct single-file r5 then reproduced the exact 37,413-key set in 6:08,
      upgraded 110 embedded parents from partial to complete, and introduced no
      completeness regression. Its 14,918 complete total differs from the
      separate restartable traversal's 14,935 by the already-observed libpff
      run-to-run classification variance; r5 retained six more writable
      properties than restartable r1. The duplicate r5 PST and the
      restartable spool/output were deleted after bounded evidence was
      retained. This closes Checkpoint 7 with exact item identity/accounting,
      independently valid direct parts, accepted ScanPST/Outlook behavior, and
      measured direct/restartable performance and write cost. The post-fix
      canonical full gate passes at
      `.agent/test-results/1784575346-full`. Direct split did not demonstrate
      a material wall-time advantage over restartable split on the current
      host: 8:37 versus 8:41. Its accepted benefits are lower write traffic
      and no mailbox-sized payload pack. Single-file direct recovery completed
      in 6:08, localizing the remaining split cost to per-candidate part
      projection. A future performance milestone must replace that projection
      with an equivalent bounded one-pass packing decision; it must not weaken
      the exact part-size or independent-validity guarantees.
  - [x] Make direct output the default for every supported PST-output recovery
    policy. Feed parser output through bounded backpressure into canonical
    translation and the transactional PST writer without first creating a
    mailbox-sized payload pack. Balanced and aggressive continue to select
    recovery breadth, not persistence. Retain a compact metadata ledger with
    source/configuration identity, bounded per-candidate status and
    completeness, part assignments, manifests, aggregate
    omission/reconstruction counts, and terminal job state so `report` and
    exact reconciliation remain reproducible.
  - [x] Keep the current durable ledger/payload-spool implementation behind an
    explicit `--restartable` flag. Permit `--resume` and `--keep-work` only in
    restartable mode and refuse incompatible combinations before creating the
    output directory.
  - [x] Write only the active PST part to a same-filesystem temporary file
    beside its destination. Complete it by construction and `fsync` it, then
    publish with atomic
    rename; never rewrite the completed dataset to move from temporary to
    final storage, including disk-to-disk recovery paths.
  - [x] Bound parser-to-writer queues, candidate metadata, attachment chunks,
    and the active transaction independently of source or attachment size.
    Preserve finalized parts on interruption. An interrupted direct job is a
    terminal partial result that remains reportable but cannot resume; a new
    run requires a different empty output directory. This prevents duplicate
    candidate publication without retaining recovered payloads.
  - [x] Report logical source bytes, bytes written to active PST temporaries,
    finalized output bytes, zero production validator reads, peak
    temporary allocation, and peak RSS. Prove the default path avoids one
    readable-mailbox-sized spool write and allocation.
  - [x] Preserve the 0.4.4 exact 37,402-to-37,402 reconciliation, one
    serialization per normal part, independent readers, ScanPST/Outlook
    interoperability, less than 2 GiB RSS, and no more than one minute per
    source GiB on the 19 GB qualification.
- [x] Milestone 0.4.6: Historical Corruption Recovery.
  - [x] Started `milestone/v0.4.6-final-correctness` in sibling worktree
    `../pstforge-worktrees/v0.4.6-final-correctness` from approved, merged, and
    pushed `main` commit `709825d`.
  - [x] Checkpoint 1: add a strict external repair-pair manifest and reusable
    privacy-safe semantic comparator. Pin corrupt source and repaired reference
    SHA-256 values; verify source identity before and after recovery through
    the production no-atime source wrapper. Compare item multiplicity and
    placement, associated and embedded ownership, visible metadata, recipients,
    bodies, recovery-critical properties, attachment metadata/payload hashes,
    and recursive content in one traversal. Existing focused/full gates retain
    exact empty-folder, folder-class, raw/named-property, and `readpst`
    coverage. Normalize only store-local identifiers and documented
    writer/ScanPST structural values. Any source-readable supplement must be
    explicitly category-pinned in the external case. Every repaired-reference
    value in that category must remain present as a multiset member; the source
    may only add demonstrably omitted values, while the repaired reference
    continues to control all other categories. Repaired-reference reads are
    strict except for the exact recognized associated-table failure. Recovering
    source-readable associated items requires an explicit `associated items`
    case supplement; its presence must exactly match observed surplus items.
    Reference matches and associated surplus consume one shared source
    multiset, so one source occurrence cannot authorize a generated duplicate,
    including when value supplementation and associated recovery occur in the
    same case. The sole repaired-reference native issue exception matches the
    complete observed 11-line diagnostic, including the checked relationship
    between folder and descriptor node IDs; substring, appended, or compound
    diagnostics fail.
    Folder identity includes the same-name sibling ordinal at every level so
    duplicate sibling display names cannot collapse. A surplus associated item
    must be in the exact ordinal-qualified folder whose repaired table is
    unreadable and must independently match one complete source-readable item;
    duplicates and invented values therefore fail. Generated catalogs admit no
    read issue, unsupported item, or incomplete item; repaired and
    damaged-supplement catalogs likewise reject unsupported items, and the
    recovery report must state zero unsupported candidates. A damaged-source
    supplement can contribute only a complete item and only the exact
    manifest-pinned category set.
  - [x] Checkpoint 2: inventory the acceptance archive into distinct semantic
    cases rather than historical attempt count. Run the smallest pairs first,
    one isolated direct one-part recovery at a time. Require internal
    validation and `pffinfo` before a one-pass repaired-reference item/content
    comparison; existing focused tests retain `readpst`, empty-folder, raw, and
    named-property coverage. Delete disposable recovered outputs after bounded
    evidence is retained. Do not request a separate human ScanPST run for
    every historical sample.
    The SHA-256-pinned external inventory contains 19 corrupt/repaired pairs.
    Sixteen pass sequential release-mode direct recovery, `pffinfo`, source
    immutability, repaired-reference item matching, and one-pass semantic
    comparison. They contain 44,465 libpff-readable repaired-reference items;
    PSTForge writes all of them plus 50 associated items recovered from the
    exact tables that remain unreadable in three ScanPST-repaired references.
    The early 0.2.1 fidelity reference drops four source-readable recipient
    rows; PSTForge retains them and otherwise matches the source under the
    documented UTF-8 normalization, so the source-readable supplement controls
    that value-level difference. Every disposable generated output was deleted
    between cases. Two synthetic OLE pairs remain separately pinned because
    their malformed `0x3701` object references use invalid NID types; ScanPST
    discards the object payload and its repaired files still expose no readable
    attachment data/type through libpff. The early MailPlus r6 source and its
    ScanPST-repaired reference both expose the same unreadable attachment table,
    so current libpff cannot prove whether the otherwise matched single item has
    complete attachment content. It is retained as a third unresolved pair
    rather than accepted through the old generic attachment-count tolerance.
    The pre-review harness state passed the
    fast gate at `.agent/test-results/1784579419-fast`; review then required
    addition-only source supplementation and explicit fallible scratch cleanup.
    The focused supplement regression and fidelity repair case pass with the
    corrections, the scratch directory is empty after the case, and the
    intermediate corrected state passed the fast gate at
    `.agent/test-results/1784579716-fast`. A later review required strict
    repaired-reference issue handling, explicit associated-item authorization,
    and shared source-multiset accounting. The 10,201-item focused case passes
    those corrections with all 25 associated extras accounted for, and the
    intermediate state passed the fast gate at
    `.agent/test-results/1784580481-fast`. A subsequent review found that
    hybrid and associated matching still used separate source multisets and
    that the native issue exception was substring-based. Both ordinary and
    hybrid duplicate-reuse regressions now pass; the exact-diagnostic regression
    rejects appended or node-mismatched errors; and the 10,201-item case passes
    again with 25 extras. The corrected state passes the fast gate at
    `.agent/test-results/1784581131-fast`. A later review found silent
    unsupported-item exclusion and best-effort cleanup on failing cases. All
    strict catalogs and the command report now reject unsupported items.
    Success and failure both pass through explicit fallible scratch cleanup;
    the forced post-write failure regression confirms its temporary data is
    removed, and the focused fidelity case remains clean with an empty scratch
    directory. The corrected state passes the fast gate at
    `.agent/test-results/1784581576-fast`. A later review correctly required
    failure-path immutability verification and 0.4.5 restartable-schema
    compatibility; both are implemented with a focused resume regression. Its
    proposed zero-omission report gate is outside the owner-approved historical
    acceptance scope: the repaired-reference item/semantic comparison controls
    this checkpoint, while existing focused/full gates retain raw-property and
    empty-folder coverage. The resulting state passes the fast gate at
    `.agent/test-results/1784582086-fast`. Review then found a pre-scratch
    immutability gap: repaired-reference catalog failure could return before
    the post-read check. Preparation now always combines with source/reference
    verification, scratch-creation failure verifies them again, and every
    post-creation result retains the existing verification-plus-cleanup path.
    The focused fidelity case and failure cleanup regression pass. Review found
    one final early repaired-reference SHA mismatch return; both manifest hash
    checks now use the pair verifier, and repaired-reference open failure
    verifies the already-open source before returning. Review then found that a
    source verification error short-circuited the repaired-reference check.
    Both checks now always execute and combine their evidence. The complete
    state passes the fast gate at `.agent/test-results/1784582785-fast`.
    Final clean-context review returned `CLEAN`. The full historical batch then
    proved 16 passing pairs and exposed MailPlus r6 as a third unresolved
    native-reader case because both source and repaired reference have the same
    unreadable attachment table. The first canonical full gate attempt at
    `.agent/test-results/1784584558-full` passed every existing stage but
    launched the new repair test without its separate manifest. A correctly
    wired full gate was
    stopped after 33 minutes when its debug-profile repair test had read about
    220 GB and only reached the first large spooled reference; scratch was
    empty after termination. The full gate now runs the external corpus in the
    release profile, matching the already proven 16.5-minute bounded batch
    instead of multiplying libpff and comparator cost in an unoptimized binary.
    The corrected automation passes the fast gate at
    `.agent/test-results/1784586891-fast`, clean-context review returned
    `CLEAN`, and the release-profile canonical full gate passes at
    `.agent/test-results/1784587009-full`. All solved corrupt/reference pairs,
    their logs, and two repaired-only remnants were then deleted from the
    external acceptance archive. The stale passing manifest and empty scratch
    directory were removed; the three-case unresolved manifest and its compact
    evidence remain. With those owner-directed private inputs removed, the
    historical test skips only when both opt-in variables are absent and
    rejects a partially configured invocation. Permanent focused/full coverage
    remains unchanged. The post-cleanup state passes the fast gate at
    `.agent/test-results/1784588212-fast`.
  - [x] Checkpoint 3: resolve each mismatch as a focused correctness change.
    Update writer conformance before writer behavior changes. Run focused tests,
    the fast gate, clean-context adversarial review, and commit/push each
    independently successful repair category. Pause for human interoperability
    only when automated evidence cannot decide correctness.
  - [x] Final gate: all 16 provable pairs pass recovery and semantic comparison,
    all sources remain unchanged, and the three current-libpff native-reader
    gaps are separately pinned for post-1.0 work. The canonical full gate passes
    before cleanup. After cleanup, the ordinary canonical full gate passes
    without historical repair variables at
    `.agent/test-results/1784588932-full`; its permanent external corpus,
    `pffinfo`, and `readpst` checks remain green while the deleted historical
    suite skips as designed. Clean-context review challenged and rejected an
    inapplicable request to hardcode the deleted private archive's 16-case and
    50-addition totals into every future opt-in manifest; no blocker or high
    finding remains. If writer changes warrant another human interoperability
    check, provide one large consolidated test-only validation PST rather than
    dozens of samples; do not expose PST merging as a product command or API.
    Merge and push 0.4.6, remove its worktree, and begin 0.5.0 only after this
    permanently closes the 0.4 series.
- [ ] Milestone 0.5.0: Operational UX and Debian Packaging.
  - [x] Started `milestone/v0.5.0-operational-ux-packaging` in sibling worktree
    `../pstforge-worktrees/v0.5.0-operational-ux-packaging` from approved,
    merged, and pushed `main` commit `ceb9478`.
  - [x] Checkpoint 1: finalize the public CLI and report contracts. Bump every
    producer/package version to 0.5.0. Implement balanced recovery verification
    without writing a job. Add `report` as a read-only consumer of a versioned
    report snapshot stored in the private SQLite ledger. Opening a report must
    not create, reconcile, compact, or otherwise mutate job state. Validate the
    ledger, manifests, published part type/identity/size, and every SHA-256 that
    the producing mode recorded. Direct mode remains deliberately hash-free and
    reports `not calculated`; do not reintroduce a post-write read. A 0.4 job
    without a report snapshot receives a clear compatibility error rather than
    fabricated zero metrics. Preserve exit statuses 0, 1, 2, 3, 4, 5, 6, and
    130 and keep final JSON isolated on stdout.
    The implementation stores a bounded digest-protected split snapshot,
    validates it against ledger-owned manifests, and opens completed SQLite
    state with immutable URI semantics after refusing active WAL state. Focused
    tests prove the read creates no files or database changes, rejects snapshot
    and finalized-part damage, regenerates equal restartable and hash-free
    direct results, explains pre-0.5 jobs, and reports balanced recovery counts.
    The checkpoint passes the fast gate at
    `.agent/test-results/1784590928-fast`. Initial adversarial review found
    that recovery verification still ran native recovery in the CLI process,
    hash-free direct parts lacked production-time filesystem identity, and the
    snapshot did not cover aggregate ledger drift or invalidate at resume
    start. Recovery verification now runs in a bounded supervised child while
    the parent holds and rechecks the protected source; an injected child abort
    returns contained source status 3 with no partial JSON. Sidecar schema 1.2.0
    records the staged file's device and inode, which survive atomic rename and
    reject a same-length replacement without hashing direct content. Snapshot
    publication also records a digest of source/configuration, recovery,
    candidate, rejection, supervision, interruption, and terminal-failure
    aggregates; resume removes every prior snapshot key before changing state.
    Focused replacement, ledger-drift, resume-invalidation, normal worker, and
    aborting-worker tests pass. The remediated checkpoint passes the fast gate
    at `.agent/test-results/1784591583-fast`. A second clean-context review
    found that abnormal recovery-verification worker exits could bypass the
    parent's final protected-source identity check, and that a report snapshot
    digest proved ledger stability without proving the snapshot's typed totals
    matched that ledger. The parent now performs its final source recheck on
    every worker result path. Report regeneration compares source,
    configuration, candidate and blob totals, recovery completion, rejection,
    supervision, interruption, terminal failure, and published-part fields to
    independently decoded ledger evidence. Direct recovery records completion
    only after the worker exits successfully. A regression test proves that a
    digest-valid but ledger-inconsistent snapshot is rejected. The twice-
    remediated checkpoint passes the fast gate at
    `.agent/test-results/1784592122-fast`. A third clean-context review found
    that a recovery-verification child lacked the parser worker's parent-death
    contract and that direct publication deferred hash-free filesystem identity
    validation until a later `report`. Verification workers now arm Linux
    `PDEATHSIG` with an expected-supervisor PID race check before native work;
    a forced-supervisor-death test proves the child cannot survive it. Both
    staged and atomically finalized direct artifacts now receive the same
    type, mode, size, device, and inode checks as restartable artifacts while
    still skipping content hashing when no SHA-256 was recorded. A same-length
    staged replacement test proves publication refuses changed identity. The
    third-remediated checkpoint passes the fast gate at
    `.agent/test-results/1784592542-fast`.
  - [x] Checkpoint 2: finalize operational reporting and privacy. Persist the
    snapshot before atomically publishing `recovery.log`; validate it against
    ledger-owned parts when regenerating human or JSON output. Add tracked JSON
    schemas and bounded fixtures for every public command. Audit diagnostics,
    progress, logs, permissions, successful spool removal, stale-partial
    cleanup, and interrupted-state reporting. Add actionable installation
    diagnostics without adding a new pre-1.0 product command.
    Six draft-2020-12 schema documents cover the shared definitions and every
    public JSON command result; five bounded synthetic fixtures deserialize
    through the production Rust report types, and the fast gate requires and
    parses every tracked artifact. The privacy contract now distinguishes
    explicitly requested source/job paths in command output from aggregate,
    path-free `recovery.log` content. Existing tests already prove private
    permissions, bounded log detail, mailbox-value exclusion, spool removal,
    secure ledger compaction, stale-partial cleanup, and interrupted-state
    durability, so the audited implementation was preserved. Missing
    restartable validators now provide actionable `pff-tools` or `pst-utils`
    package hints only for `ENOENT`; direct mode retains no dependency on
    either tool. The checkpoint passes the fast gate at
    `.agent/test-results/1784593183-fast`. Clean-context review found the
    initial verify schema capped issue detail at 1,000 while the parser retains
    10,000, and found that syntax-only schema checks neither resolved `$ref`
    targets nor validated fixtures. The bound now matches the implemented
    10,000-entry limit. Portable relative schema identifiers resolve against
    adjacent installed files, while xtask builds an offline local registry,
    compiles every command schema under draft 2020-12, resolves all references,
    and semantically validates the matching fixture. The remediated checkpoint
    passes the fast gate at `.agent/test-results/1784593513-fast`. The next
    review trace showed that missing validator executables were hidden behind
    the supervised child exit and could not reach the initial hint mapping.
    Restartable split now preflights executable `pffinfo` and `readpst` entries
    in `PATH` before source or output work, returns a typed missing-validator
    error, and prints the corresponding Ubuntu/Debian package hint. Direct mode
    does not run the preflight. Focused tests cover both available tools, each
    missing-tool mapping, and absent `PATH`. The final remediated checkpoint
    passes the fast gate at `.agent/test-results/1784593850-fast`. The bounded
    final review correctly noted that mode bits alone do not establish whether
    the current process can execute a validator. The preflight now combines a
    regular-file check with the OS effective-access `X_OK` decision, and its
    test rejects a non-executable candidate. Public schema enums and the verify
    fixture were also narrowed to the exact modes and inventory scopes emitted
    by production. The resulting fast gate passes at
    `.agent/test-results/1784594021-fast`.
  - [x] Checkpoint 3: add reproducible Debian packaging. `cargo xtask package
    deb` builds the release binary with `--locked`, stages only declared files,
    installs the binary, manual, README/operator documents, application
    licenses, writer MIT notice, and libpff LGPL notice, then creates a
    root-owned reproducible `amd64` package. Runtime dependencies include the
    Debian-compatible `libpff1t64 (>= 20180714)` floor and only binary ABI
    dependencies; development readers are not runtime dependencies. Inspect
    ownership, modes, paths, control metadata, dynamic linkage, reproducibility,
    installation, command execution, and removal without touching user jobs.
    `cargo xtask package deb` now builds the locked release binary, derives its
    glibc and libgcc ABI dependencies through `dpkg-shlibdeps`, retains the
    documented `libpff1t64 (>= 20180714)` compatibility floor, strips only the
    staged binary, and produces a root-owned `amd64` package. It installs the
    CLI, manpage, README/product specification, public JSON schemas, application
    licenses, adapted-writer MIT license, and dynamic-libpff LGPL notice. Two
    isolated Cargo target directories use disabled incremental state, stable
    source dates, and path remapping; their independently staged archives must
    compare byte-for-byte before the package is published. Validation requires
    the exact declared path set, modes,
    metadata, version execution, dynamic `libpff.so.1` without RPATH/RUNPATH,
    lintian with error failure, and isolated `dpkg` install/removal that leaves
    an operator-job sentinel intact. On Ubuntu 26.04 the package SHA-256 is
    `770405a66dce211ddaf83d7aefaf47d67d2929cb15ee76837a87ea8c9bcdf1c4` and
    its generated dependencies are `libc6 (>= 2.39), libgcc-s1 (>= 4.2),
    libpff1t64 (>= 20180714)`. The checkpoint passes the fast gate at
    `.agent/test-results/1784594750-fast`. Adversarial review found that the
    first implementation could follow a symlinked workspace `target`, only
    restaged one compiled binary, and assigned the final package name before
    validation. Cleanup now refuses a symlinked/non-directory `target` (with a
    sentinel-preservation regression test), both binaries are compiled
    independently, and validation precedes the final atomic rename. The
    remediated checkpoint passes the fast gate at
    `.agent/test-results/1784595017-fast`. Final review then identified missing
    notices for Rust code linked into the executable. Packaging now walks only
    the locked, Linux, normal-dependency closure, installs each crate's complete
    shipped license/notice files with identity, version, authors, expression,
    and source, and synthesizes an attributed MIT grant only when a crate ships
    no text but its expression explicitly offers MIT. Unknown missing-license
    cases fail closed; an xtask-only dependency exclusion test guards closure
    accuracy. The final bundle covers 89 executable dependencies and lintian
    remains clean. The licensing-remediated checkpoint passes the fast gate at
    `.agent/test-results/1784595555-fast`.
  - [x] Checkpoint 4: replace the stale README with current features,
    limitations, basic usage, source compilation, Ubuntu development packages,
    Debian installation/removal, exit statuses, privacy, recovery modes, and
    restartability tradeoffs. Run the fast/full/release gates on Ubuntu 26.04
    and execute the package against Debian 13 libraries or a clean Debian 13
    environment. Record any host package prerequisite by its verified Ubuntu
    package name. No MailPlus or Outlook check is required unless this milestone
    changes PST writer behavior.
    README now covers supported data classes, direct/restartable behavior,
    hard limitations, safe output handling, basic commands, output layout,
    exit statuses, privacy, external-corpus testing, verified Ubuntu 26.04
    build/development packages, source compilation, and Debian package
    installation/removal. Focused review corrected the private manifest path
    and restored documented usage status 2 in both README and manpage. The fast
    documentation gate passes at `.agent/test-results/1784595852-fast`.
    The first release attempt exposed stale canonical-manifest paths for five
    focused fixtures intentionally removed after acceptance and a determinism
    assertion that compared run-specific published inode/device fields. Full
    gates now regenerate six documented writer fixtures only in a
    temporary directory, update a temporary structured manifest, fail closed
    for any other missing case, and include a deterministic five-object OLE
    source matching the removed human fixture's structural contract. Cross-run
    comparison omits only published filesystem
    identity while each persisted sidecar still must match its real artifact.
    The canonical release gate then passed all external tests, `pffinfo`,
    `readpst`, licenses, advisories, writer acceptance, docs, and the locked
    release build at `.agent/test-results/1784596839-release`. Adversarial
    review rejected an intermediate design that skipped the missing human OLE
    case; the six-fixture generator closes that gap without repopulating the
    acceptance archive. The final
    reproducible package has SHA-256
    `770405a66dce211ddaf83d7aefaf47d67d2929cb15ee76837a87ea8c9bcdf1c4`.
    In an ephemeral bubblewrap tmpfs, the packaged binary loaded with signed
    Debian 13 (trixie) `libc6`, `libgcc-s1`, `libpff1t64`, `libbfio1`, and zlib,
    reported version 0.5.0, and completed `info --json` against the external
    healthy Unicode PST. The temporary compatibility root disappeared on exit.
- [ ] Milestone 0.5.1: GitHub CI and Private-Corpus Automation.
  - [x] Checkpoint 1: bump every package/operator version marker to 0.5.1 and
    add a local `ci` gate that covers the public formatting, check, Clippy,
    tests, documentation, licenses, workflow contracts, and independent writer
    validation without requiring private PSTs. The first local CI gate passed
    at `.agent/test-results/1784597651-ci`.
  - [x] Checkpoint 2: add branch-push Ubuntu 24.04 and Debian 13 automation,
    scheduled RustSec and bounded parser fuzzing, a manually dispatched private
    self-hosted corpus gate with no artifact/cache upload, and an approved-tag
    Debian build that is gated by the `release` environment and does not
    publish a GitHub release. Third-party actions are pinned to full commits;
    the downloaded `actionlint` 1.7.12 archive is pinned by SHA-256.
  - [ ] Checkpoint 3: pass clean-context adversarial review, commit and push the
    branch, prove the hosted public lanes, configure the GitHub `release`
    environment with owner approval, run the exact private-manifest full gate,
    then merge the approved milestone without a progress-only commit.
    The repository `release` environment now requires review by the
    `calculatetech` owner and limits deployments to `v*`; self-approval remains
    possible because this single-owner repository has no second release
    operator. The exact canonical-manifest full gate passed at
    `.agent/test-results/1784597956-full`, including RustSec, regenerated
    focused writer fixtures, the external ANSI/Unicode corpus, `pffinfo`, and
    `readpst`. A clean isolated Rust 1.85 toolchain exposed a newer-compiler
    dependency selection and unstable syntax that the Rust 1.93 host had
    hidden. The lock now selects the latest compatible ICU 2.1 family, and
    equivalent nested control flow plus generic sink dispatch remove those
    language-version dependencies. The final complete CI gate passed under the
    real Rust 1.85 compiler at `.agent/test-results/1784598619-ci`, and the
    exact canonical-manifest full gate passed after the final workflow
    hardening at `.agent/test-results/1784599078-full`. The final fresh
    clean-context adversarial review reported `CLEAN`. Hosted branch execution
    remains open; no private runner is registered yet.
- [ ] Milestone 0.6.0: Interoperability Release Candidate.
- [ ] Milestone 1.0.0: MailPlus-Ready Release.

## Surprises & Discoveries

- Observation: A package-level `rust-version = "1.85"` declaration does not
  stop a lock generated by a newer Cargo from selecting newer transitive
  dependencies, and the development host compiler can hide newer syntax in
  existing code. The initial 0.5.1 lock selected ICU 2.2 (`rust-version =
  "1.86"`), while several let-chains and trait-object upcasts also failed on
  Rust 1.85. An isolated, checksum-verified Rust 1.85 toolchain was necessary
  to expose both classes of failure before the hosted lanes ran.
  Evidence: failing isolated 1.85 checks followed by the passing complete CI
  gate at `.agent/test-results/1784598619-ci`.

- Observation: The first default-direct 19 GB qualification reached the full
  writer pass with no mailbox-sized payload spool, but all three attempts
  failed with `direct worker message has no embedded catalog identity`.
  Unsupported embedded descendants remained valid protocol ownership facts
  even though they were intentionally excluded from PST output; the previous
  spooled-tree read model omitted them. Retry also requires the same binding
  facts beneath an already-written top-level root before its replay is drained.
  A dedicated durable-catalog traversal supplies both cases without loading
  mailbox-wide identity state.
  Evidence: `.agent/test-results/1784531559-v045-direct-single-19gb`,
  focused terminal-state regression, and the clean-context review completed on
  2026-07-20.

- Observation: libpff's item identifier is stable enough for top-level PST
  nodes but is not a cross-process identity for an embedded message. For the
  same source parent node 3283844 and attachment zero, three independent
  traversals reported embedded IDs 3075377693, 3408225810, and 3283876.
  Parent containment and attachment position remained identical. Direct replay
  must therefore use the durable containment relationship plus recovery
  provenance, not the embedded node ID, while retaining collision rejection.
  Evidence: r2/r3 ledger comparison and
  `.agent/test-results/1784533130-v045-direct-single-r3/stderr.log`.

- Observation: Direct metadata and payload traversal need the same native
  embedded-item acquisition order, but not the same event nesting. On r4,
  source-order metadata committed nested parent `normal:283401542:-:0` as
  partial with no attachment events, while all three writer-order passes found
  its child under attachment zero after the active PST had reached 656 MiB.
  Recursive metadata events cannot be sent to the single-active durable sink.
  Acquiring and queueing the child before attachment-property decoding, then
  emitting it only after the parent closes, preserves both readability and the
  flat transactional catalog.
  Evidence:
  `.agent/test-results/1784533900-v045-direct-single-r4/stderr.log`, read-only
  r4 ledger inspection, and focused clean-context reviews on 2026-07-20.

- Observation: A libpff embedded `PffItem` can outlive the Rust attachment
  wrapper enough to expose its header while later child attachment access
  fails after that container is freed. Retaining the owning attachment beside
  queued child work exposed 13 additional nested messages on the 19 GB source
  and cleared the prior direct replay mismatch without retaining payload bytes.
  The measured peak remained about 316 MiB, well below the 2 GiB acceptance
  ceiling. The pending traversal already retained one child item per queued
  embedded attachment; the container handle changes the constant factor, not
  the existing source-controlled breadth.
  Evidence: `.agent/test-results/1784536000-v045-direct-single-r6`, r5/r6
  ledger comparison, and controlled `SIGTERM` at 19:07.

- Observation: Exact per-message final-size projection became the dominant
  cost in a large single direct PST. Each candidate caused the writer to plan
  final folders, tables, and B-trees for all messages accepted so far, and the
  real append repeated that projection. This is O(n²) CPU work even though
  payload writing itself is sequential. During r6 one supervisor core remained
  saturated while active-PST throughput decayed below 3 MiB/s on a host with
  idle multi-GiB/s NVMe capacity. One whole-part projection followed by one
  streaming write reduces this planning work to O(n) for the single-output
  qualification without increasing payload retention or write amplification.
  Evidence: `.agent/test-results/1784536000-v045-direct-single-r6`, process and
  disk telemetry from that run, and the whole-part direct projection
  regression.

- Observation: A valid Unicode SIBLOCK cannot be made deeper to accommodate a
  large table: MS-PST fixes `cLevel` at 1 and defines SIENTRY as a pointer to an
  SLBLOCK. The 19 GB single-store projection instead exposed inefficient HNID
  selection: small contents-table strings and identifiers were all stored as
  subnodes even though each fits a normal HN heap allocation. Using documented
  HIDs for those values preserves the same table cells while avoiding the
  finite subnode namespace and reducing structural/message-size overhead.
  Evidence: `.agent/test-results/1784538080-v045-direct-single-r7`, MS-PST
  sections 2.2.2.8.3.3.2.1, 2.2.2.8.3.3.2.2, 2.3.1, and 2.3.4.4.2, plus the
  173,600-small-value writer regression.

- Observation: The 19 GB source's global `libpff_file_recover_items` call
  fails after normal reachable traversal. Metadata recovery correctly retains
  the complete reachable catalog and records that parser failure, but
  writer-order replay formerly required a generic success frame after
  streaming the same entire catalog. In r8 that mismatch discarded one
  complete 19.53 GB unpublished PST and began writing it again. Durable-catalog
  exhaustion is the scientific completion boundary for replay: accepting a
  parser boundary before exhaustion would lose cataloged data, while requiring
  libpff success after exhaustion adds no data and causes pure write
  amplification.
  Evidence: `.agent/test-results/1784539095-v045-direct-single-r8`, the r8
  19,530,195,968-byte exact projection, and the direct parser-boundary
  acceptance/rejection regression.

- Observation: A completed-store validator must use the exact NAMEID catalog
  used to write the store, not reconstruct it from successfully serialized
  values. The durable source catalog intentionally retains identities whose
  values are omitted as damaged or unsupported so every later named-property
  ID remains stable. Reconstructing from output messages removes those
  reserved identities and makes valid later properties appear absent or
  mismatched. Qualification r10 isolated this at output message node
  `0x0031AE44`; no invalid PST was published.
  Evidence: `.agent/test-results/1784541051-v045-direct-single-r10` and
  `transactional_validation_retains_unused_source_named_property_ids`.

- Observation: A libpff record entry is not independently owned by its Rust
  wrapper. Retaining the raw entry after freeing its record set caused a native
  `memmove` crash at source candidate 24,966. Reopening the entry avoided the
  invalid pointer but did not provide an independent cursor: a previously read
  64 KiB prefix remained consumed in libpff's shared stream state, so the
  34,342-byte remainder of a 99,878-byte property was absent. The safe,
  single-read model is to retain the record-set owner with the original entry
  until its remainder drains. Attachment handles are reacquired only because
  libpff exposes an explicit attachment-data seek API.
  Evidence: the r6 native core backtrace, isolated full-worker completion in
  180 seconds after the ownership correction, focused document/OLE round
  trips, and qualification r10.

- Observation: One-pass direct construction can begin before the traversal has
  discovered later empty folders. The initial part layout therefore had zero
  reportable source folders even though the completed ledger contained 164
  source folder records and the output retained 145 representable folders.
  Using the initial count made the otherwise completed r9 part fail sidecar
  integrity and correctly prevented publication. Publication accounting must
  use the final observed folder set, while the writer continues to observe
  late folders before finalization.
  Evidence: r9 ledger counts, the sidecar integrity refusal, and r10's
  atomically published 145-folder manifest.

- Observation: A folder node derived from the sorted set of paths currently
  known to a streaming writer is not stable. Adding a lexicographically earlier
  path shifts every later index even though previously written message rows
  retain their old parent node. In r10, final folder properties described the
  correct source counts while the contents rows belonged to different folders;
  shared empty-table selection then understated two BBT reference counts.
  Append-stable first-observation node assignment fixes the causal ownership
  error. A finalization-time row/plan equality check is required so any future
  identity divergence blocks publication without relying on output rereads.
  Evidence: human r10 ScanPST log, including folder count swaps, BBT entries 14
  and 34, and orphan recovery; focused late-folder and injected-shift
  regressions; qualification r11.

- Observation: The 0.4.2 fidelity expansion regressed a cold 19 GB split from
  approximately ten minutes in 0.4.1 to more than 57 minutes without
  finalizing the second part. Recovery durably cataloged 37,399 normal items,
  29,168 attachments, 310 embedded messages, and 3,469,174 raw properties, but
  packing then held up to 6,293,820 KiB RSS and rewrote near-4-GiB candidate
  parts repeatedly. Part 0002 reached at least its fifth full serialization
  attempt. SIGTERM did not stop the CPU-bound writer within 30 seconds and the
  owner authorized SIGKILL. The source device, inode, size, modification time,
  and SHA-256 remained unchanged; finalized part 0001 and durable job state
  remained intact.
  Evidence: bounded timing and `pidstat` evidence under the untracked
  `.agent/test-results/v042-final-qualification/` directory. The killed command
  exited 137 after 57:47.52 with 3,288.08 user CPU seconds, 192.82 system CPU
  seconds, and 6,293,820 KiB maximum RSS. Finalized part 0001 is 4,289,520,640
  bytes and contains 10,226 messages.

- Observation: The incremental 0.4.3 retained-job run published four new
  parts while using 321,888 KiB peak RSS and no swap, proving that PST payload
  bytes are streamed to the private PST rather than retained in process RAM.
  Parts 0002 through 0004 are between 4,276,315,136 and 4,293,075,968 bytes;
  final part 0005 is 2,227,430,400 bytes. The private publication directory
  peaked at 4,363,942,890 bytes while validating part 0005, about 69 MB above
  the part target. Atomic publication is a same-filesystem no-replace rename
  and does not rewrite payload bytes. The durable payload pack remains a real
  additional readable-payload write and capacity cost.
  Evidence: external `v043-retained-resume-r5.log`,
  `v043-retained-resume-r6.log`, and
  `v043-retained-resume-r6-peak-bytes.txt`.

- Observation: Exact finalized-size projection after every message was a
  serialized CPU bottleneck, not an NVMe throughput limit. During the 19 GB r1
  baseline the source and output devices stayed roughly 7-20% utilized with
  effectively zero I/O wait while the parser/supervisor consumed about one CPU
  core. The run completed in 11:03.95 at 321,676 KiB maximum RSS, but first
  publication took about 6:49 and part 1 spent about 104 seconds in append and
  boundary work. The adaptive transactional r2 append reached the identical
  part-1 byte boundary in about 15 seconds and published it in about 34.5
  seconds after writer start, placing first publication about 5:20 after
  invocation.
  Evidence: external `v043-spooled-r1.log`, `v043-spooled-r1.json`,
  `v043-spooled-r2.log`, and live `pidstat`/`iostat` sampling on 2026-07-19.

- Observation: A provisional batch can contain a candidate that translation
  durably marks unsupported. If a later candidate makes that batch exceed the
  part limit, rewinding the candidate cursor across the terminal candidate and
  trying to load it as spooled fails ledger validation. Retaining the terminal
  candidate indexes for exact replay makes the rewind skip those already
  accounted candidates. A focused 320 KiB split now forces this shape and
  proves that every later valid message is published.
  Evidence: external r2 failure at 5:46 after part 1 publication and
  `split_contains_unrepresentable_candidate_and_writes_later_mail`.

- Observation: Two fresh libpff traversals of the same corrupt 19 GB source
  produced the same 37,402 committed candidates and 37,035 written candidates,
  but differed by 10 complete/partial classifications and 24 readable blobs.
  Part message counts remained identical; the later traversal omitted one
  fewer attachment but 10 additional properties, making part 4 about 4.8 MB
  smaller. This is recovery nondeterminism rather than a writer-boundary
  difference. A third fresh traversal reproduced r1 exactly, including all
  recovery counts and every part SHA-256, establishing that adaptive writer
  batching itself is byte deterministic while retaining the anomalous r2
  evidence for later recovery-layer investigation.
  Evidence: external r1/r2/r3 JSON comparison; source identity and SHA-256
  match.

- Observation: Human ScanPST evidence and a repaired comparison PST placed
  beside part 0001 exposed two independent facts. First, part 0001 from the
  prior 0.4.2 implementation fails ScanPST and requires a later focused
  correctness comparison; it is not evidence against the new transactional
  parts and is deferred until the performance path is stable. Second, the
  resume validator incorrectly treated unrelated public `parts/` files as
  private-ledger corruption. Public evidence and comparisons are now ignored,
  preserved, and excluded from capacity credit; only private `.pstforge/`
  storage remains exclusive.
  Evidence: external `parts/part-0001.log` and
  `parts/part-0001-repaired.pst`.

- Observation: Classic Outlook compressed RTF can include a final NUL in the
  header's `RAWSIZE`. The `compressed-rtf` decoder deliberately removes that
  terminator, so comparing the decoded code-unit count directly to `RAWSIZE`
  rejects valid Outlook data. Accepting an exact one-code-unit difference is
  bounded: any earlier NUL would leave a larger difference. Omitting this RTF
  left its method-6 OLE payloads structurally present but removed the
  `\objattph` body relationships Outlook needs to display them.
  Evidence: Outlook-authored `embedded email.pst`, source `PR_RTF_COMPRESSED`
  length 15,844 with `RAWSIZE` 56,138, decoded size 56,137, source
  `pffexport` RTF output, and the failed ScanPST-clean r4 Outlook acceptance
  on 2026-07-19.

- Observation: ScanPST rejects NID types `31` and `1` for the raw-data subnode
  referenced by a method-6 `PidTagAttachDataObject`, even though MS-PST defines
  the object descriptor only as a subnode NID and does not publish a dedicated
  type relationship. Both repairs deleted the OLE 2 attachment rather than
  providing a corrected type. A ScanPST-clean classic-Outlook specimen contains
  five method-6 `PtypObject` attachments whose descriptors all use reserved
  NID type `0x09`, making that repeated real-writer behavior the narrowest
  supported interoperability rule.
  Evidence: `qualification-v042-ole-r1`, `qualification-v042-ole-r2`, their
  logs and repaired PSTs, plus Outlook-authored `embedded email.pst` SHA-256
  `99fc6e28ca18900f54c9411cbbcd5ef6a29fa2e6e1c5b0fd2e0b573411c15f48`
  and the owner's clean ScanPST result on 2026-07-19.

- Observation: `PidTagAttachRendering` is WMF data and has no documented
  16-KiB ceiling. Applying the small-property materialization bound to
  `0x3709`, `0x3702`, and `0x370A` caused avoidable omission of readable
  metadata above that size.
  Evidence: checkpoint-11c clean-context review and the 20-KiB streamed
  rendition regression on 2026-07-19.

- Observation: A zero-length leaf data block is not a valid way to externalize
  an empty OLE value; the independent PST reader rejects its block trailer with
  `cb = 0`. `PtypBinary` can retain the exact empty value inline, while a
  `PtypObject` descriptor has no valid empty data subnode to reference.
  Evidence: the checkpoint-11c empty-payload regression and independent
  completed-store validation on 2026-07-19.

- Observation: libpff exposes a method-6 `PtypObject` property descriptor
  separately from the attachment payload. The property stream itself can be
  empty while the object descriptor points to a data subnode whose logical
  bytes are returned through the attachment data API. Treating the empty
  property stream as the payload would silently lose a valid OLE 2 object.
  Evidence: the bounded `v042-ole-attachments-source` libpff catalog and exact
  source/output payload hashes on 2026-07-19.

- Observation: `PidTagAttachTag` is not method-6-only in real data. The public
  split corpus contains a method-1 JPEG with a readable nine-byte binary attach
  tag. Adding `0x370A` to the exact external fingerprint exposed PSTForge's
  prior omission; preserving `0x3702`, `0x3709`, and `0x370A` on complete
  by-value attachments restored the exact public source/output comparison.
  Evidence: `.agent/test-results/1784467959-full` and the corrected focused
  `milestone_0_4_real_pst_splits_deterministically_without_mutation` run.

- Observation: `PidTagAttachDataObject` type validation must occur only after
  method `6` dispatch. Embedded-message attachments can expose a writer-managed
  `0x3701` relationship without a scalar property type in synthetic canonical
  input; validating it eagerly marked an otherwise clean embedded message
  partial. The fast gate caught the regression.
  Evidence: `reports_invalid_filetimes_and_clean_embedded_mail_are_contained`
  and `.agent/test-results/1784467707-fast`.

- Observation: libpff's attachment convenience APIs recognize method `2` as a
  reference but reject methods `3`, `4`, and `7`, then attempt to stream data
  from every data-less reference. The raw attachment property set still
  exposes `PidTagAttachMethod` and all path/NAMEID values correctly. Reading
  that method before content dispatch avoids false parser issues and avoids
  any request for nonexistent or external content.

- Observation: The first by-value Document candidate preserved its attachment
  byte-for-byte, but the synthetic DOCX was itself invalid because its OPC ZIP
  omitted the package-level `_rels/.rels` relationship. ScanPST correctly
  passed because this was payload validity, not PST corruption. The replacement
  fixture includes the normative officeDocument relationship, has a focused
  package-structure test, and exports with the same payload hash before and
  after splitting.

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

- Decision: Public CI runs on pushes to `main` and `milestone/**`, not pull
  requests. Private corpus automation is manual on a dedicated self-hosted
  runner, accepts its manifest path only through a GitHub secret, and uploads
  no artifacts, caches, detailed logs, PSTs, spool state, or mail metadata.
  Release automation accepts an already-existing tag only when it equals `v`
  plus the checked-out package version, requires the protected `release`
  environment, builds a retained package artifact, and never creates or
  publishes a GitHub release.
  Rationale: This repository deliberately does not use pull requests, while
  private mailbox evidence must remain on the controlled host. Read-only
  workflow permissions, immutable action pins, bounded summaries, explicit
  dispatch, and environment approval provide useful automation without making
  GitHub a data store or granting automation authority to release software.
  Date/Author: 2026-07-20 / project owner workflow policy and Codex.

- Decision: The 0.4.6 historical-pair gate accepts documented partial-success
  output when item/content comparison against the repaired reference succeeds
  and no item is classified as unsupported. It does not require every pair to
  report zero omitted folders, properties, or attachments.
  Rationale: The owner explicitly limited this historical archive gate to
  repaired-reference item counts and automated semantic comparison, with
  existing focused/full tests retaining exact empty-folder, raw-property,
  named-property, and independent-reader coverage. The early fidelity fixture,
  for example, preserves the approved behavior while reporting six source
  property omissions. Turning this checkpoint into universal raw-property
  equivalence would contradict that acceptance decision and reopen completed
  writer design work. Unsupported items remain prohibited because they can
  disappear from both the semantic catalog and the output count.
  Date/Author: 2026-07-20 / project owner item-count acceptance direction and
  Codex review-scope challenge.

- Decision: Retain the two historical synthetic OLE invalid-NID pairs and the
  early MailPlus r6 unreadable-attachment-table pair as the only unresolved
  0.4.6 native-reader cases, and defer them with the libpff fork until after
  1.0. Remove solved corrupt/repaired pairs and their paired
  ScanPST logs from the external acceptance archive only after the 0.4.6
  harness checkpoint passes review and repository gates.
  Rationale: Both corrupt objects encode `PidTagAttachDataObject` through NID
  types that are invalid under the PST contract. libpff cannot retrieve their
  attachment type or data, and ScanPST repair discards rather than recovers the
  payload. MailPlus r6 has one otherwise matched item, but both source and
  repaired reference fail the same libpff attachment-table read, so neither can
  serve as a complete attachment oracle. Later real-source OLE preservation
  already passes; retaining these three specimens supplies focused evidence for
  post-1.0 native salvage work without preserving dozens of solved
  multi-gigabyte artifacts.
  Date/Author: 2026-07-20 / project owner cleanup and libpff deferral direction
  with automated comparison evidence.

- Decision: Historical repair pairs use automated semantic content and item
  count comparison plus independent readers; they do not each require human
  ScanPST or Outlook work. If a writer correction requires final human
  interoperability evidence, provide one large consolidated test-only
  artifact where feasible. This does not add PST merging to the pre-1.0
  product.
  Rationale: Existing acceptance already establishes the output structure.
  Repeating dozens of manual structural scans adds little evidence, while
  automated pair comparison directly tests the milestone's data-preservation
  purpose.
  Date/Author: 2026-07-20 / project owner.

- Decision: Defer any PSTForge-maintained libpff fork until after 1.0. Keep
  using replaceable dynamic linking to the system library for the 1.0 product.
  Retain evidence of traversal classification variance, corrupt-tail behavior,
  and native recovery limitations as the input to that later investigation.
  Rationale: The accumulated evidence justifies focused upstream/native parser
  work, but current containment and accounting satisfy the pre-1.0 recovery
  contract. Forking the LGPL component now would expand correctness,
  distribution-source, and maintenance scope without closing the remaining
  application-level acceptance work.
  Date/Author: 2026-07-20 / project owner.

- Decision: Default direct output performs one supervised libpff traversal and
  one destination construction. It buffers only one top-level message graph's
  bounded metadata prefixes, commits parent metadata before embedded
  descendants, then streams unread payload remainders from the retained native
  handles. It does not retry a failed direct worker because reconstructing an
  unpublished one-pass stream would require rereading source content. A worker
  failure aborts the active ledger transaction, removes the unpublished PST,
  preserves finalized parts, and returns a terminal partial result.
  `--restartable` remains the deliberate choice for durable replay and retry.
  Rationale: Direct mode exists to minimize source reads, SSD write
  amplification, elapsed time, and retained state. Claiming transparent retry
  while restarting traversal would contradict that contract and can duplicate
  catalog occurrences. Bounded one-graph metadata is sufficient for canonical
  translation and hard part selection without mailbox-scale RAM.
  Date/Author: 2026-07-20 / project owner performance direction and Codex
  implementation evidence.

- Decision: Native deferred-property ownership follows libpff's actual
  lifetime and cursor behavior. A record entry retains its record-set owner
  and continues from the bounded prefix on the same handle. It is neither
  reopened nor reread. Message items remain alive for the graph lifetime.
  Attachment data may use a freshly acquired attachment handle only with an
  explicit checked seek to the already captured prefix. Duplicate
  `PR_ATTACH_DATA_BIN` property data is not read a second time when the same
  bytes are available through the attachment stream.
  Rationale: The 19 GB source supplied both a deterministic use-after-owner
  crash and a shared-cursor counterexample to reopen-based continuation.
  Retained RAII ownership is bounded to one top-level graph, preserves the
  one-read direct contract, and matches the native API's proven semantics.
  Date/Author: 2026-07-20 / qualification evidence and Codex.

- Decision: Close 0.4.2 at the owner-approved focused data-correctness
  boundary and move the failed final 19 GB scale reconciliation to an immediate
  0.4.3 performance milestone. Do not represent the aborted run as passing.
  Preserve the one finalized part and matching durable job temporarily as a
  0.4.3 performance fixture; delete it after bounded replacement evidence
  exists so failed runs do not consume permanent disk space.
  Rationale: Each 0.4.2 data-type checkpoint passed its independent and human
  interoperability gates, while the aggregate run exposed a different,
  blocking implementation defect. Keeping that defect in an unbounded
  validation loop delays useful recovery and obscures the already accepted
  correctness checkpoints.
  Date/Author: 2026-07-19 / project owner and Codex.

- Decision: Version 0.4.3 will write each part incrementally and make the fit
  decision only for the next indivisible top-level message. It will not
  materialize the complete mailbox before publication, determine the total
  recoverable byte count before writing, or repeatedly serialize whole
  candidate parts to discover their size. Track actual allocated PST bytes and
  bounded finalization headroom; finalize the current part when the next
  message cannot fit. Only one indivisible oversize message may exceed the
  target.
  Rationale: PST files are procedurally constructed. The current immutable
  full-store specification and prefix-calibration loop turn a bounded packing
  choice into multiple multi-gigabyte rewrites. Source safety, independent
  validation, atomic publication, and durable recovery remain non-negotiable,
  but they do not require this work amplification.
  Date/Author: 2026-07-19 / project owner and Codex.

- Decision: Use bytes allocated in the private PST, not source-payload bytes or
  a fixed message count, as the primary cheap projection trigger. Far from the
  boundary, perform exact finalized-size projection after approximately
  `part_size / 16` new private bytes. Within the final `part_size / 16`, reduce
  the batch target to `part_size / 256`. Keep a 2,048-message ceiling only as a
  replay-latency guard for unusually small items. Accept a batch only after an
  exact projection; roll back an over-limit batch byte-for-byte and replay it
  individually to select the last fitting message.
  Rationale: Source and output lengths differ because property contexts,
  tables, block trailers, alignment, allocation maps, and final NBT/BBT pages
  are reconstructed. Their exact final form need not be rebuilt after every
  append. The policy scales to a 64 MiB split (about 4 MiB far batches and 256
  KiB near batches) and a 4 GiB split (about 256 MiB and 16 MiB), while exact
  projection remains the hard-limit authority. Observed 4 GiB finalization
  overhead is about 32 MiB, so the 256 MiB near window retains substantial
  measured headroom.
  Date/Author: 2026-07-19 / project owner direction and measured 0.4.3
  qualification.

- Decision: Treat performance and interruption behavior as release
  correctness. On the current high-end host, a cold split must finish within
  one minute per source GiB and stay below 2 GiB aggregate RSS. There is no
  separate first-part deadline. Restartable mode must stop at a durable
  boundary promptly on SIGINT or SIGTERM, and resuming a materially progressed
  job must be faster than restarting it from the beginning. Qualification must
  measure cold and resume runs and must not leave failed scratch output after
  evidence is retained.
  Rationale: Faster hardware masks rather than fixes serial work amplification.
  A restartable design is defective when replaying its persistence state costs
  more than repeating source recovery.
  Date/Author: 2026-07-19 / project owner and Codex; performance target
  clarified by the owner after final 0.4.4 qualification.

- Decision: Version 0.4.5 makes low-write direct writing the default for every
  supported PST-output recovery policy and keeps restartable persistence
  behind explicit `--restartable`.
  `--resume` and `--keep-work` require that flag. Direct mode keeps a compact
  metadata ledger and manifests for exact accounting and `report`, but never a
  recovered-payload pack. If interrupted, its published parts and reporting
  state form a terminal partial job; continuing requires a new empty output
  directory rather than risking duplicates. Each active PST temporary lives
  beside its destination and becomes the final file by atomic rename, so
  publication never copies or rewrites the completed part.
  Rationale: Restart persistence is useful but imposes a mailbox-sized extra
  write and temporary allocation. Making that SSD cost deliberate preserves
  the recovery option without charging every run. Compact accounting remains
  necessary for trust, privacy-safe reporting, and one-to-one reconciliation.
  Date/Author: 2026-07-19 / project owner direction and closeout review.

- Decision: Use one exact whole-part projection when the requested part limit
  can contain the source file; otherwise retain exact incremental split
  projection until a separately qualified batched boundary algorithm replaces
  it.
  Rationale: A recovered PST cannot exceed the source-size limit check without
  first failing this fast eligibility test, so the optimization cannot weaken
  the requested hard maximum. The projection consumes only bounded catalog
  metadata and writer allocation state, opens no payload streams, and writes
  no PST blocks. Actual output still streams once to a same-filesystem
  temporary, is compared with the projected final EOF, independently
  validated, synced, and atomically published. This removes the measured O(n²)
  planning defect for the immediate single-file acceptance while leaving the
  already-qualified 4 GiB split boundary behavior unchanged.
  Date/Author: 2026-07-20 / measured r6 evidence and Codex implementation.

- Decision: For default-direct splitting, use exact private allocation as the
  per-candidate admission signal and reserve the final `part_size / 16` for
  exact finalized-size projection. This scales the exact region to every
  supported limit rather than hard-coding a 4 GiB batch. Each admitted append
  must reproduce its projected private EOF. Finalization remains an independent
  exact calculation and refuses an over-limit multi-message part before atomic
  publication if a pathological final-index ratio exceeds the boundary
  reserve.
  Rationale: The rejected 4 GiB r1 and r2 runs showed that rebuilding every
  folder table and NBT/BBT plan for each candidate is quadratic CPU work even
  when the second rebuild is removed. The measured finalization overhead for a
  4 GiB part is about 32 MiB, while the proportional boundary is 256 MiB.
  Candidate-private projection uses the writer's real block allocation without
  reading payload data. The final exact refusal preserves the hard maximum
  without making a heuristic authoritative.
  Date/Author: 2026-07-20 / measured r1-r2 evidence and Codex implementation.

- Decision: Reference attachment classification comes from the readable
  `PidTagAttachMethod` property, not libpff's narrower convenience type API.
  Preserve methods `2`, `3`, `4`, and `7` as data-less relationships and never
  dereference their path/URL. Any readable conflicting
  `PidTagAttachDataBinary`, including an empty value, is a counted
  damaged-source omission; it is not allowed to silently change the reference
  into a by-value attachment. Durable replay preserves reference and missing
  terminal states separately, so surviving properties cannot promote an
  incomplete attachment to a complete reference.
  Rationale: Current OXCMSG documents methods `2`, `4`, and `7`; Outlook's
  canonical MAPI property documents legacy method `3`. The external fixture
  proves that libpff exposes their raw relationship properties even when its
  convenience API rejects the method. Preserving source semantics without I/O
  is both the documented behavior and the safest recovery boundary.
  Date/Author: 2026-07-18 / Codex and project owner after clean r2
  interoperability acceptance.

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
  starting a fresh restartable split. Direct mode excludes the absent payload
  spool, uses a mode-specific output/active-part/validator estimate, and
  rechecks observed allocation before each part. A matching restartable resume
  credits the validated allocation already consumed by that job against the
  same conservative total. Report invocation time, logical source/final output bytes,
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

- Decision: Treat one minute per source GiB as the operational acceptance
  target for qualification on this host, in addition to the existing 2 GiB RSS
  and correctness gates. Emit phase progress during recovery so the absence of
  finalized parts before traversal completes is distinguishable from a stall.
  Rationale: The utility is needed for immediate recovery work, and a nominal
  24-hour ceiling does not satisfy the owner's stated usable turnaround.
  Date/Author: 2026-07-17 / project owner and Codex; target clarified by the
  owner after the final 0.4.4 run.

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

- Decision: Job schema 16 admits `IPM.DistList` descendants and preserves
  structurally readable PSETID_Address member arrays without interpreting
  their EntryID payloads. A readable primary member array is authoritative;
  an absent optional one-off mirror stays absent, an inconsistent mirror is
  omitted alone, and an unusable primary also removes its dependent mirror and
  checksum. The source checksum is retained only when the source member bytes
  are retained unchanged and is never recomputed.
  Rationale: MS-OXOCNTC defines primary and optional synchronized
  `PtypMultipleBinary` properties below 15,000 bytes. Schema 15 catalogs can
  already contain these values classified as omitted, so resume must fail
  closed. Byte preservation maximizes recovery without claiming that damaged
  One-Off or Wrapped EntryIDs can be safely reconstructed.
  Date/Author: 2026-07-18 / Codex from checkpoint-10 Microsoft conformance and
  bounded source/output fingerprint evidence.

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

- Decision: A fully automated conformance recovery point may be committed and
  pushed without a separate approval message after its required gate and fresh
  final-state review pass, provided it changes no accepted PST bytes, requires
  no empirical product choice, and needs no ScanPST, Outlook, or MailPlus
  evidence. Any writer-byte change, unresolved empirical disposition, or
  human interoperability step still pauses before commit.
  Rationale: Small normative documentation and regression checkpoints provide
  clean recovery points but do not benefit from an idle approval round trip
  when all acceptance evidence is machine-verifiable.
  Date/Author: 2026-07-18 / human owner direction.

- Decision: Retain EMP-01 through EMP-10 as required interoperability output,
  and retain EMP-13 as an exact source-preservation exception. Where a
  published description conflicts with demonstrated real-world behavior,
  proven ScanPST and Outlook behavior controls.
  Rationale: Current candidates pass both independent validators; earlier
  omissions produced concrete HMP, search-folder, hierarchy, and contents-table
  repair findings; several structures have no published replacement; and
  EMP-13 preserves readable source properties exactly. Rebuilding, omitting,
  or normalizing these values would introduce known or unbounded
  interoperability and preservation risk without improving recovered data.
  Date/Author: 2026-07-18 / human owner approval of the structural empirical
  checkpoint and reality-over-conflicting-specification rule.

- Decision: Retain the source-derived values in EMP-14 and EMP-15 as permanent
  recovery policy. Derive folder class from a readable message class, copy the
  readable half of sender and recipient identity pairs, use a valid delivery
  time when creation/modification time is absent, and derive an associated
  display name from its readable subject.
  Rationale: These transformations preserve usable source facts and improve
  imported-object identity without fabricating unrelated information. Leaving
  the destination fields empty would discard available context, while
  independent defaults would be less trustworthy. Typed `recovery.log` counts
  keep every derivation visible without exposing source values.
  Date/Author: 2026-07-18 / human owner approval of the derived-value
  checkpoint.

- Decision: Leave a wholly missing subject and sender identity absent across
  readable message classes instead of writing `(no subject)` or
  `Unknown Sender`. Continue copying one readable sender half to its missing
  counterpart across message classes, require a separate nonempty display name
  for associated items, and count every applicable absent subject, sender name,
  and sender address in the bounded typed `recovery.log`. When an associated
  item has neither display name nor subject, retain the existing neutral
  `(no subject)` value only for its required associated display name and count
  that generated field separately.
  Rationale: ScanPST accepted both comparison messages. Outlook leaves the
  omitted list fields blank and supplies its own `(no subject)` view label;
  MailPlus supplies `(No subject)` while leaving the sender blank. In contrast,
  MailPlus renders the fabricated sender as `<Unknown@SYNTAX_ERROR>`. Omission
  preserves the source truth and lets each client apply its native display
  policy without turning invented metadata into a malformed identity.
  Date/Author: 2026-07-18 / human owner approval after Outlook and MailPlus
  comparison.

- Decision: Treat code page 65001 as derived only when missing-source HTML is
  nonempty and passes full-stream strict UTF-8 validation, and treat
  `message/rfc822` as derived only from an actual method-5 embedded Message
  object.
  Rationale: UTF-8 validation provides deterministic content evidence, with
  nonempty ASCII remaining byte-identical under the modern compatible code
  page. An empty property supplies no evidence about its encoding.
  Embedded messages have no by-value attachment prefix to sniff, but their
  structural attachment method is definitive. Other missing MIME types and
  ambiguous legacy HTML require a separate corpus-backed detector and remain
  generated or absent.
  Date/Author: 2026-07-18 / human owner direction to prefer tested decoding
  evidence and the most modern compatible encoding.

- Decision: Infer a missing by-value attachment MIME type only from an exact,
  format-defined leading signature whose media type is unambiguous.
  Rationale: PDF, PNG, JPEG JFIF, GIF, and classic TIFF are distinguishable
  within eleven bytes and do not require filename trust or payload
  materialization. ZIP and OLE identify containers rather than their contained
  document type, while arbitrary text and legacy code pages admit multiple
  interpretations. Those cases remain unlabeled rather than receiving a
  plausible but unproven value.
  Date/Author: 2026-07-18 / human owner direction to use data decoding when
  tests establish a clear winner.

- Decision: Preserve every complete unknown attachment byte stream and use a
  deterministic `.bin` recovery filename only when its source filename is
  absent or empty. Classify common Office attachments from bounded
  container structure, using a source extension only as correlated evidence:
  exact ZIP is generic ZIP unless one supported OPC main content type and
  required part agree or a recognized OOXML extension survives independently;
  CFB is DOC, XLS, or PPT when one corresponding root stream has a valid format
  marker or agrees with its source extension. Extension alone never classifies
  arbitrary bytes, and proven content overrides a conflicting extension.
  Rationale: Users need the original payload for later forensic recovery even
  when PST metadata and the payload's own directory are damaged. A generic
  container label communicates useful evidence without claiming document
  integrity. The 256 MiB structural-inspection cap exceeds Exchange Online's
  documented configurable 150 MB maximum message size and Gmail's 25 MB
  personal attachment limit, while containing parser memory and CPU on hostile
  input; larger attachments remain byte-exact and receive only signature-level
  classification. ZIP entry, central-directory, XML-size, XML-event, and CFB
  entry limits further bound corrupt-container work.
  Date/Author: 2026-07-18 / human owner direction to cover common Office/ZIP
  documents and retain unknown data for later recovery analysis.

- Decision: When an attachment has no nonempty source filename, generate a
  deterministic extension from the strongest supported type evidence. A
  payload-proven type controls the generated extension even when preserved
  source MIME metadata conflicts; otherwise a recognized source MIME supplies
  the extension. Use `.bin` when neither proves a supported type and `.msg` for
  an embedded Message object. Never alter a nonempty source filename.
  Rationale: A usable extension improves recovered-file handling without
  modifying payload bytes or presenting an arbitrary guess. Content evidence
  is stronger than potentially damaged metadata for a newly generated display
  value, while preserving the original MIME property keeps the source fact
  available for later analysis.
  Date/Author: 2026-07-18 / human owner approval to use the correct extension
  when possible.

- Decision: Preserve method-6 OLE attachments from the readable source
  relationship: keep `0x3701` as `PtypObject` or `PtypBinary`, stream its exact
  complete bytes, and retain readable `0x370A`, `0x3702`, and `0x3709` binary
  values including an explicitly empty rendition. Preserve those same readable
  binary metadata properties on complete by-value attachments. Do not require
  the payload to parse as a valid OLE container and never instantiate, execute,
  repair, convert, infer, or dereference it.
  Rationale: The property type is the normative distinction between OLE 2
  storage and OLE 1 OLESTREAM data. Container validity is useful fixture
  evidence but making it a recovery prerequisite would discard precisely the
  damaged objects PSTForge is intended to salvage. Bounded streaming and exact
  hashes preserve source facts without interpreting hostile content.
  Date/Author: 2026-07-19 / implementation decision from Microsoft OXCMSG and
  canonical-property contracts, pending human interoperability acceptance.

- Decision: Encode a method-6 `PtypObject` data subnode with the reserved
  Outlook-observed NID type `0x09`, and stream readable attachment encoding, rendering, and
  attach-tag properties when they exceed the small-property materialization
  threshold.
  Rationale: MS-PST requires an object descriptor to reference a subnode but
  does not publish a dedicated NID type for generic OLE data. ScanPST rejects
  both raw-LTP type `31` and internal type `1`, and its repairs delete the
  attachment payload. A ScanPST-clean classic-Outlook PST uses type `0x09` for
  each of five independently stored object payloads. The owner approved
  observed reality when the published specification is silent. WMF rendition
  data has no documented 16-KiB ceiling, so a memory convenience bound cannot
  become a data-recovery loss policy.
  Date/Author: 2026-07-19 / ScanPST r1/r2 evidence, Outlook-authored comparison,
  and human approval to proceed.

- Decision: Preserve a complete zero-byte method-6 `PtypBinary` inline, contain
  zero-byte `PtypObject` data as malformed, and include every inline raw
  attachment-property payload in aggregate preflight sizing.
  Rationale: Empty binary is an exact source value and has a valid inline PST
  encoding. A zero-length external data block fails independent PST validation,
  so an object descriptor cannot truthfully reference it. Counting all raw
  metadata prevents near-limit attachments from passing preflight and failing
  later during whole-message construction.
  Date/Author: 2026-07-19 / clean-context review followed by focused writer
  regression and independent completed-store evidence.

- Observation: Parser traversal order is not a durable message identity.
  Metadata traversal may defer embedded messages while writer traversal emits
  them recursively, so occurrence counts over a mixed traversal can bind a
  corrupt duplicate to the wrong payload. Direct streaming now resolves a
  child from its already-resolved durable parent plus attachment index and
  source identity; only top-level duplicates use their stable catalog
  occurrence.
  Date/Author: 2026-07-20 / clean-context adversarial review and focused
  duplicate-parent protocol regression.

- Observation: `Read` errors from a worker pipe and I/O errors writing the
  destination PST share the writer's public I/O error variant. Without a typed
  inner marker, a mid-payload native crash bypasses parser retry while a broad
  retry rule would incorrectly retry disk failures. Direct payload readers now
  wrap only worker-stream I/O with a private typed marker; output I/O remains a
  terminal output failure.
  Date/Author: 2026-07-20 / clean-context adversarial review and injected OLE
  payload abort.

- Decision: Make low-write non-restartable streaming the default PST-output
  execution mode for every supported recovery policy. Balanced and aggressive
  select source-recovery breadth, not persistence. Add `--restartable` as the
  deliberate opt-in for durable payload recovery; `--resume` resumes only such
  a job and `--keep-work` is invalid otherwise. Direct mode still retains
  compact SQLite accounting, but no
  payload pack. Deleting a spool after success does not
  reduce device writes and therefore is not an acceptable implementation of
  streaming mode. Both modes build each PST under a private name on the output
  filesystem, complete it by documented construction, `fsync` it once, and
  publish it with an
  atomic no-replace rename that never falls back to a cross-filesystem copy.
  Report the selected mode plus conservative estimated and measured temporary
  writes/disk use. Direct mode retains bounded per-candidate accounting,
  current-part indexes, and bounded current-item state, but it must not
  materialize an entire attachment, PST, or mailbox in RAM or a mailbox-sized
  payload spool. An interrupted direct job is terminal and reportable; rerun
  requires a new empty output directory.
  Rationale: Restartable spooling writes every readable payload once before
  writing it again into PST output. That recovery guarantee is useful but can
  impose roughly dataset-sized extra SSD writes and capacity, materially
  affecting QLC endurance on 19 GB, 50 GB, and 83 GB recovery jobs. The owner
  requires that tradeoff to be explicit rather than the default.
  Date/Author: 2026-07-19 / human owner direction after measured 0.4.3
  retained-job write amplification; clarified 2026-07-20 for every supported
  recovery policy. The first scale acceptance is one direct PST from the 19 GB
  source with exact 1:1 content accounting and clean ScanPST/Outlook results.

- Decision: Direct mode performs one source-content traversal and one
  destination PST construction. It does not pre-hash source bytes, rehash
  output bytes, reopen output to rebuild allocation maps, or invoke internal or
  independent PST readers before publication. The writer must emit final NBT,
  BBT, allocation maps, density list, header, and all client relationships
  correctly as bytes land. It then performs one file `fsync`, an atomic
  no-replace rename, and a directory `fsync`. Independent validation remains a
  mandatory CI, adversarial-review, ScanPST, Outlook, and MailPlus acceptance
  layer, not a production transformation stage. Restartable mode retains
  source and payload SHA-256 because persisted state must be matched on a later
  invocation.
  Rationale: Runtime self-reading cannot make a structurally incorrect writer
  trustworthy and multiplied the 19 GB job's reads and elapsed time. A direct
  job has no resume state whose identity requires a content digest.
  Date/Author: 2026-07-20 / human owner direction after the 19 GB r11 timing
  analysis.

- Decision: Hold the source through one read-only, no-follow descriptor and
  request every applicable Linux read-side protection: a kernel read lease,
  whole-file open-file-description record read lock, and shared file lock. A
  process-scoped POSIX record lock is used only when OFD locking is unavailable.
  A lease-break signal interrupts the supervised job. Unsupported lease
  semantics are reported and fall back to the advisory protections; a
  conflicting lock or existing writer is a refusal. Recheck descriptor/path
  device, inode, size, mtime, and ctime before publication in all cases.
  Rationale: A read lease prevents new write opens on supporting local
  filesystems, while OFD/flock locks protect against cooperating software.
  Linux cannot make advisory locks constrain an arbitrary writer, so unchanged
  identity remains the final mixed-snapshot guard.
  Date/Author: 2026-07-20 / human owner direction.

- Decision: A direct full-payload worker failure receives at most three clean
  parser attempts. Finalized parts and terminal candidate classifications are
  drained on a later attempt; the unpublished active part is discarded and
  rebuilt. If all attempts fail, preserve finalized output and compact ledger
  state, emit a typed `failed-partial` report and recovery log, refuse resume,
  and require a new empty output directory.
  Rationale: Direct mode deliberately avoids mailbox-sized restart state, but
  native parser faults must remain contained and observable. Replaying only
  the current unpublished part gives bounded fault recovery without duplicating
  published candidates or reintroducing payload-spool write amplification.
  Date/Author: 2026-07-20 / clean-context adversarial review.

- Decision: In restartable mode, hash the complete source once at invocation
  open, match that hash before trusting resume state, and use
  held-descriptor/path identity including Linux ctime for the completion
  recheck instead of rereading the entire source for a second SHA-256. Direct
  mode does not perform this pre-hash.
  Rationale: PSTForge never writes through the held read-only descriptor. Any
  filesystem-mediated content or metadata write changes ctime even when an
  actor restores the original size and mtime. Comparing device, inode, size,
  mtime, and ctime on both the held descriptor and pathname therefore detects
  an in-run source change without another 19/50/83 GB read. Full SHA-256 remains
  mandatory before restartable recovery and on every later invocation before a
  durable job is trusted.
  Date/Author: 2026-07-19 / 0.4.3 measured resume optimization under the
  repository's source-identity recheck contract; narrowed 2026-07-20 by human
  owner direction to restartable execution.

- Decision: On durable open, hash-verify payload blobs only when at least one
  `pending`, `spooled`, or `failed` candidate can still consume them. Continue
  validating the pack inode, bounds, non-overlap, ledger relationships, every
  finalized PST hash, and every private-state path. A later candidate that
  attempts to reuse a consumed blob must verify that exact range before reuse.
  Rationale: Blobs referenced exclusively by `written` candidates have already
  been incorporated into independently validated, hashed, atomically published
  PSTs. Blobs referenced exclusively by `unsupported` candidates cannot enter
  output. Rehashing those mailbox-sized private ranges on every resume adds
  SSD reads without protecting any future write.
  Date/Author: 2026-07-19 / 0.4.3 retained-job resume profiling.

- Decision: Treat `readpst` 0.6.76's rejection of a PC BTH with nonzero
  `bIdxLevels` as a reader-specific validation limitation, not a reason to
  remove or flatten the documented multi-level property context. Continue to
  require `readpst` for structures it supports and record the unsupported
  message explicitly. Require the normative MS-PST contract, focused
  round-trip tests, clean ScanPST, and successful Outlook consumption for the
  multi-level form.
  Rationale: The reader accepts the same message as a compact PC and accepts
  external PCs with a level-0 BTH, but rejects the documented
  `cbKey=2`, `cbEnt=6`, `bIdxLevels=1` header before reading any record.
  ScanPST reports no error and Outlook consumes both the message and its
  properties. A third-party reader's missing BTH-level implementation cannot
  become an artificial PSTForge data-loss boundary.
  Date/Author: 2026-07-19 / human acceptance after focused r5 comparison.

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

Version 0.4.2 completed its focused data-correctness series through small,
independently verifiable checkpoint commits. Recursive embedded attachments,
native item classes, associated data, document/reference/OLE representations,
missing metadata behavior, and the conformance remediation set passed their
required automated gates and owner-run ScanPST and Outlook checks. The final
cold 19 GB reconciliation was stopped by owner direction after 57:47 because
only one part had finalized, part 0002 had reached at least its fifth full
serialization attempt, RSS reached 6,293,820 KiB, and SIGTERM was not observed
within 30 seconds. The source remained byte-for-byte and identity-metadata
unchanged and finalized work survived SIGKILL. Therefore 0.4.2 closes on its
accepted correctness evidence with the scale reconciliation explicitly
incomplete. Version 0.4.3 immediately owns the blocking writer, memory, resume,
and cancellation performance defects before any operational UX or packaging
work begins.

Version 0.4.4 closes the whole-job data-reconciliation gap. Five focused
checkpoint commits preserved explicit empty bodies, scaled recipient tables
and message property contexts, recovered every remaining readable candidate,
and replaced empty rejection events with strict privacy-safe structural
categories. The final current-code 19 GB run completed in 9:30.47, about
30 seconds per source GiB, at 323,200 KiB maximum RSS. It assigned all 37,402
unique readable candidates exactly once across five independently validated
parts, with no unsupported, failed, stranded, duplicate, or unassigned item.
The source SHA-256 and identity remained unchanged. The three attachment
omissions correspond exactly to source `attachment_missing` events. The owner
accepted the current automated reconciliation and prior ScanPST/Outlook
evidence as completing the version.

Version 0.4.5 now defaults every supported PST-output recovery policy to
bounded one-traversal direct construction. Human ScanPST rejected
qualification r10 because late folder discovery shifted folder nodes already
referenced by streamed message rows. The corrected r11 assigns append-stable
folder nodes and refuses final row/plan disagreement before publication. It
wrote the current 19 GB source into one 19,533,751,296-byte PST in 6:55.18 at
462,420 KiB peak RSS. It assigned all 37,413 currently readable candidates
exactly once, left `payload.pack` at zero bytes, omitted no attachment or
folder, published no digest, and retained the unchanged source identity plus
one known contained libpff recovery-tail issue. Record entries retain their
native record-set owners through deferred streaming and attachment
continuations use checked native seek. The human owner reported a clean
ScanPST result and successful Outlook use of r11. `--restartable` deliberately
retains the existing durable ledger and payload spool; its performance
optimization and the 4 GiB direct regression remain later 0.4.5 checkpoints
after single-file acceptance.

Version 0.4.6 closes the historical corruption archive with a strict,
privacy-safe repaired-reference harness. Sixteen provable pairs recover 44,465
repaired-reference items plus 50 exact source-matched associated items; every
case passes internal validation, `pffinfo`, semantic multiplicity/content
comparison, and source/reference immutability checks. Clean-context review is
clean and the canonical release-profile full gate passes at
`.agent/test-results/1784587009-full`. Three compact cases remain explicitly
unresolved for the post-1.0 libpff fork: two malformed OLE object-reference NID
pairs and MailPlus r6, whose source and repaired reference share an unreadable
attachment table. The owner-directed cleanup deleted solved corrupt/reference
pairs, paired logs, repaired-only remnants, the stale passing manifest, and
scratch. The unresolved manifest and those three compact evidence sets remain.
No writer behavior changed, so no further human ScanPST or Outlook validation
is required for this milestone.

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
      [--restartable] [--resume] [--keep-work] [--json]
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
Outlook, and MailPlus. The first cold attempt was stopped after 57:47 with only
one finalized part because whole-part trial serialization made the validation
operationally intolerable. Preserve that failed-gate conclusion and complete
this reconciliation under milestone 0.4.3 after the incremental writer meets
its performance budgets. The 50 GB source remains a later release gate.

### Milestone 10: Version 0.4.3 - Incremental Writer Performance

Replace whole-mailbox canonical materialization and repeated whole-part trial
serialization with a bounded transactional stream. Recovery may continue to
append payload bytes and canonical metadata durably, but the writer consumes
completed top-level messages in deterministic order as they become eligible.
It appends one indivisible message at a time to the current PST, tracks actual
allocated bytes plus a measured upper bound for finalization structures, and
finalizes before the next message would cross the requested target. It does
not need the total readable mailbox size before writing. A message larger than
the target remains the sole permitted oversize case.

Introduce a writer transaction boundary that can either commit one complete
message or discard only that uncommitted message. Persist the last committed
source candidate, part index, writer allocation state, and payload-pack cursor
often enough that resume does not replay completed source recovery or rebuild
already finalized parts. Make every data-dependent writer and validator loop
observe the shared interruption flag at bounded intervals. SIGINT and SIGTERM
must stop assignment, preserve finalized parts, remove only known partial
scratch, checkpoint durable state, and exit with status 130 promptly.

Prove the architecture first with deterministic fake-backend tests that force
part boundaries, oversized messages, interruption during append and finalize,
disk exhaustion, and resume from every transaction boundary. Add instrumentation
for source hashing, recovery, canonicalization, append, finalization, and
validation, including bytes read/written, CPU time, wall time, peak aggregate
RSS, trial-write count, and time to first finalized part. A normal part must be
serialized exactly once; validation reads are permitted but must not rewrite
it.

Use the retained interrupted 0.4.2 job only as a temporary private performance
fixture while developing the writer path. Once focused proof exists, perform a
cold run against the named 19 GB manifest case. Acceptance requires completion
within one minute per source GiB with the previous approximately ten-minute run
as the optimization target, less than 2 GiB aggregate RSS, no repeated
whole-part trial writes, prompt graceful interruption, and a materially faster
resume than cold restart. Independently validate every part and reconcile the
readable source inventory against the split output and explicit omission
record. Delete failed private scratch after retaining bounded evidence. Do not
begin the next milestone until this gate passes or the owner records a specific
exception.

Implement checkpoint 1 in `crates/pstforge-pst/src/writer.rs`. Define
`TransactionalMailStoreWriter` as the sole owner of its private temporary file,
allocation cursor, emitted block records, next BID/NID counters, compact folder
rows, compact validation expectations, and interruption reference.
`TransactionalMailStoreWriter::append_message` accepts one folder identity,
placement, and `MessageSpec`. Before mutation it records the file length,
allocation cursor, ID counters, and vector lengths. It writes that message's
blocks exactly as the existing batch loop does, then computes the exact
projected final extent by applying the documented block/page alignment and
NBT/BBT page capacities to the retained state. `rollback_message` truncates to
the recorded file length and restores every counter/vector length; it is legal
only for the most recent uncommitted append. `finalize` writes folder and
template blocks, BBT, NBT, allocation maps, and header once, then uses the
existing internal validation, independent-reader supervision, sync, and
no-clobber publication path. The existing `create_mail_store*` functions become
compatibility wrappers over this API and must retain byte-for-byte output for
the same input.

Named property IDs are store-wide, so `begin` receives a sorted,
duplicate-free named-property identity catalog before the first message.
Checkpoint 1 derives it from the existing batch input. Checkpoint 2 obtains it
through one bounded ledger query over named-property descriptors; it must not
read property payloads or construct messages. Including a mapping that a
particular part does not use is valid and gives every output part the same
deterministic source-wide identity-to-ID assignment.

Implement checkpoint 2 in `crates/pstforge-job/src/lib.rs`,
`crates/pstforge-core/src/canonical.rs`,
`crates/pstforge-core/src/writer_input.rs`, and
`crates/pstforge-core/src/split.rs`. Add an ordered top-level candidate cursor
that returns one candidate and its recursively owned descendants with only
their events. Do not call `spooled_candidates_interruptible`,
`candidate_ownerships_interruptible`, or `load_canonical_mail_interruptible`
from the production split path. Translate the returned tree with a
single-message function that returns the `MessageSpec`, all durable item keys,
omission/reconstruction accounting, and compact folder placement. Feed it to
the transactional writer immediately. If appending it would make the exact
projected final extent exceed the target and the part already has a message,
roll back only that candidate, finalize and publish the current part, begin the
next part, and append the candidate there. If it alone exceeds the target,
finalize it as the documented oversize part.

The current prefix estimator, adaptive calibration, whole-candidate
`write_staged_part` attempts, and complete `BTreeMap<ItemKey, CanonicalMail>`
materialization leave the production path after checkpoint 2. Focused unit
tests may retain pure packing estimators as test substitutes, but no real split
may serialize a normal part more than once. Instrument an exact
`part_serializations` counter and fail the 19 GB gate if it exceeds the number
of finalized parts plus documented oversize retries.

Checkpoint 3 may persist only deterministic, reconstructible writer state. A
finalized part and its ledger publication transaction remain the primary
restart boundary. If interruption occurs inside an unpublished part, discard
that known partial file and replay only its candidates from the ledger; never
reparse or retransmit candidates assigned to finalized parts. This is expected
to be faster and safer than attempting to deserialize live Rust writer
internals. Observe the interruption flag at least once per candidate, per
streamed 64 KiB blob chunk, per BBT/NBT page, per allocation-map page, and
before every sync, validation process, hash pass, and rename.

### Milestone 11: Version 0.4.4 - Exact Recovery Reconciliation

Close the readable-source accounting gap exposed by the 19 GB qualification.
Reconcile every recoverable source candidate exactly once against finalized
output or a bounded, reason-coded omission. Preserve valid descendants when a
damaged parent cannot be emitted, reject unexplained count differences, and
distinguish source corruption from writer loss. Acceptance is exact automated
accounting against the readable source inventory plus clean independent scans
of representative output.

### Milestone 12: Version 0.4.5 - Direct Construction and Performance

Make one-pass direct construction the default for every supported recovery
mode. Remove redundant output verification and hashing from the direct path,
hold an operating-system read lock where available, refuse output-name
conflicts, and retain restartable spooling only behind an explicit durability
choice. Acceptance uses the 19 GB source to prove exact content accounting,
clean ScanPST and Outlook behavior, bounded memory, and the one-minute-per-GiB
performance target.

### Milestone 13: Version 0.4.6 - Historical Corruption Archive

Compare historical damaged cases to their ScanPST `-repaired` references,
never to the damaged sources as the completeness oracle. Permit a
manifest-pinned source supplement only when it proves an exact readable item
that ScanPST omitted. The completed archive proves 16 cases containing 44,465
reference items plus 50 exact source-only associated items. Three cases remain
unprovable because current `libpff` cannot read the malformed OLE references or
attachment table in either the damaged input or repaired reference; retain
only those bounded cases for a post-1.0 reader fork. Remove passing private
samples after recording bounded evidence, and keep the historical harness
strict but opt-in when its private manifest is available.

### Milestone 14: Version 0.5.0 - Operational UX and Debian Packaging

Finalize report schemas, privacy redaction, exit statuses, error wording,
report regeneration, install diagnostics, spool cleanup, and operator docs.
Create a reproducible Debian 13 x86_64 `.deb` that dynamically depends on
`libpff1t64` at the Debian-compatible floor. Run build and integration tests in
a clean Debian 13 environment and on Ubuntu 26.04.

Implement the milestone through four independently recoverable checkpoints.
First finalize the command surface: balanced `verify --mode recovery`, stable
exit mapping, and a `report` command backed by a versioned snapshot in the
private ledger. Report opening is strictly read-only. It validates durable
state and every owned output's identity and length; it checks a part digest
only when the producing mode recorded one. This preserves the approved direct
mode contract, which deliberately avoids hashing or rereading completed PSTs.
Jobs created before 0.5.0 that lack a snapshot fail with an explicit
compatibility diagnostic rather than invented metrics.

Next finalize bounded human/JSON output, schemas, privacy, spool/partial
cleanup, and operator diagnostics. Then add an `xtask` Debian builder whose
staging tree contains only the release binary, manual, operator documents, and
required license/notices. Normalize timestamps from `SOURCE_DATE_EPOCH`, use
root ownership in the archive, declare the Debian-compatible dynamic
`libpff1t64 (>= 20180714)` dependency, and build twice to prove byte-for-byte
reproducibility. Package tests inspect paths, permissions, ELF dependencies,
control metadata, install/remove behavior, and preservation of a pre-existing
user job directory.

Finally replace the stale README with current features, limitations, usage,
compilation, Ubuntu dependency installation, Debian package installation and
removal, exit statuses, privacy boundaries, and direct-versus-restartable
tradeoffs. Validate locally on Ubuntu 26.04 and against Debian 13 runtime
libraries or a clean Debian 13 environment. No writer behavior changes in this
milestone, so human ScanPST, Outlook, and MailPlus testing is not a gate.

Acceptance: installing the package on clean Debian 13 makes `pstforge info`,
`verify`, `split`, and `report` work with only declared dependencies; removing
it leaves user jobs and source PSTs untouched; package contents, licenses, and
dependency metadata pass inspection.

### Milestone 15: Version 0.5.1 - GitHub CI and Private-Corpus Automation

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

Implement this milestone without changing recovery or writer behavior. Add a
public `cargo xtask gate ci` tier that is independent of all private PSTs and
runs formatting, locked workspace checks, warnings-as-errors Clippy, tests,
private-item documentation, license policy, generated writer acceptance, and
independent `libpff`, `pffinfo`, and `readpst` checks. Keep `full` as `ci` plus
RustSec and the explicitly configured external corpus; keep `release` as
`full` plus the locked release build and reproducible Debian package.

On pushes to `main` and `milestone/**`, run the public gate on Ubuntu 24.04 and
build/validate the package in Debian 13. Validate all workflow YAML with a
checksum-pinned `actionlint`, pin every referenced action to a complete commit,
and grant only `contents: read`. Schedule separate RustSec audits for the main
    and fuzz dependency locks and a five-minute, 256 KiB-bounded native Rust
    PST-reader fuzz target. The fuzz input must be ephemeral generated bytes, never a private
mail-derived corpus committed or uploaded by automation.

The private corpus workflow is manual, runs only from `main` on a self-hosted Linux x64
runner carrying the `pstforge-private-corpus` label, and obtains the exact
manifest path from `PSTFORGE_CORPUS_MANIFEST` in repository secrets. It may
write detailed evidence locally under ignored `.agent/test-results`, but must
not invoke artifact or cache upload actions. Its remote output is a bounded
tier/result summary with an explicit no-upload statement.

The release build is also manual. It checks out an existing operator-supplied
`v*` tag, proves that exact tag points at `HEAD` and equals `v` plus the
checked-out `pstforge-cli` package version, enters the protected `release`
environment, reruns the public CI gate, builds the reproducible Debian package,
and retains that package as a short-lived workflow artifact. It cannot create
a tag, GitHub release, package publication, push, or merge. Acceptance requires
local workflow validation, clean hosted Ubuntu and Debian runs from the
milestone branch, confirmed release-environment enforcement, the exact private
manifest full gate on the controlled host, and clean adversarial review.

### Milestone 16: Version 0.6.0 - Interoperability Release Candidate

Freeze CLI/schema changes, run every local and GitHub gate, fuzz parsers and
writer structures, inject disk and process failures, and complete security,
license, privacy, and data-loss reviews. Import representative parts into a
clean MailPlus test mailbox and compare folder/message counts and sampled
content. Open the same parts in supported Outlook as a secondary test.

No blocker or high-severity review finding may remain. Medium findings must be
fixed or explicitly accepted by the human owner in the Decision Log. A release
candidate is not 1.0 until the real 50 GB rehearsal succeeds from clean state.

### Milestone 17: Version 1.0.0 - MailPlus-Ready Release

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

Revision note (2026-07-20): Closed the 0.4.6 historical corruption archive
after proving 16 repaired-reference cases, recording three current-libpff
exceptions for post-1.0 work, completing the release-profile full gate, and
removing private passing samples. The strict historical harness is now opt-in
when its external manifest and scratch directory are explicitly supplied.
