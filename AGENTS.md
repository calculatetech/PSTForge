# PSTForge Agent Instructions

These instructions apply to the entire repository. The authoritative product
behavior is in `docs/PRODUCT_SPEC.md`, the version roadmap is in
`docs/ROADMAP.md`, and the living implementation plan is in
`.agent/EXECPLAN.md`. Maintain the ExecPlan in accordance with
`.agent/PLANS.md`.

## Product Priority

PSTForge 1.0 is a Linux-native command-line utility that reads a damaged PST
without modifying it and creates smaller, independently valid Unicode PST
files for import into Synology MailPlus Server. Outlook compatibility is a
secondary interoperability check.

Preserve every readable native PST item and useful property that Outlook,
MailPlus, or another PST reader could consume. This includes non-mail item
classes, hidden associated items, folder/store metadata, named properties, and
recursive embedded attachments. Do not add EML, Maildir, PDF, GUI, OST
conversion, PST merging, in-place repair, password cracking, date-range
partitioning, or folder-based partitioning before 1.0 unless the human owner
changes the product specification.

## Non-Negotiable Source Safety

- Open every source PST read-only. Never request or emulate write access.
- Never repair, truncate, rename, move, delete, or overwrite a source PST.
- Refuse a source that resolves inside the output job directory, and refuse
  symlinked source files rather than following them.
- Record the source device, inode, size, modification time, and SHA-256 before
  recovery. Recheck identity metadata before reporting completion.
- Write generated data only beneath the selected output job directory.
- Create temporary output beside its final destination and publish it with an
  atomic rename only after validation and `fsync` complete.
- Treat every value from `libpff` and every source property as untrusted.
- A native parser crash, damaged item, unsupported property, or failed
  attachment must be contained, recorded, and must not corrupt completed work.
- Never report an item as written until its destination PST part has passed
  validation and has been atomically finalized.

## Version Policy

Implementation starts at `0.1.0` and follows the milestone versions in
`docs/ROADMAP.md`.

- A new feature category increments the minor digit and resets the patch digit,
  for example `0.3.1` to `0.4.0`.
- A related feature in the current category increments the patch digit, for
  example `0.4.0` to `0.4.1`.
- Fixes discovered before a milestone commit stay at that milestone's version;
  do not consume another version while the work is still uncommitted.
- A defect discovered after an approved version marker becomes a new patch
  milestone.
- `1.0.0` is created only after every release acceptance gate in the ExecPlan
  passes.
- Keep the workspace package version, CLI version, report schema producer
  version, Debian package version, and milestone documentation consistent.
- Version 0.4.2 uses multiple reviewed checkpoint commits on one milestone
  branch. Keep every checkpoint at version 0.4.2; do not create progress-only
  commits or treat a checkpoint commit as a new version marker.

## Branches And Worktrees

Never implement code directly on `main`.

1. Start each milestone from the last human-approved `main` commit.
2. Create branch `milestone/vX.Y.Z-<slug>`.
3. Create a sibling worktree at `../pstforge-worktrees/vX.Y.Z-<slug>`.
4. Make all code, test, manifest, build, and package changes in that worktree.
5. Keep one milestone version for all checkpoint work and pre-commit fixes in
   the worktree.
6. Remove the worktree only after its branch is approved and integrated.

Documentation-only changes use a `docs/<slug>` branch. The initial
`docs/roadmap` branch is the bootstrap exception for this unborn repository;
after human approval its commit may become the initial `main` baseline without
a merge commit.

Do not create a milestone branch until the previous milestone is approved.
Do not push, merge, tag, publish, or rename the default branch without explicit
human approval. Approval for a commit is not approval to push or merge.

The repository has a reachable GitHub remote at
`git@github.com:calculatetech/PSTForge.git`, with `main` as the approved baseline
and default branch. Add GitHub Actions, branch protection, release environments,
badges, remote forks, or other dependent work only from an approved milestone.
Local automation must always work independently of GitHub. The `gh` CLI is
authenticated as `calculatetech` with `repo` scope; recheck `gh auth status`
before an API operation rather than assuming credentials remain unchanged.

## Review And Commit Gate

Uncommitted work is the review unit. Before every commit:

1. Run the automation tier required by `.agent/EXECPLAN.md`.
2. Inspect the complete diff, including generated or vendored files.
3. Perform an adversarial review focused on source immutability, native FFI,
   integer bounds, path traversal, crash recovery, data loss, PST validity,
   privacy, licensing, and missing tests.
4. Resolve all blocker and high-severity findings. Resolve medium findings or
   record an explicit human-approved exception in the ExecPlan Decision Log.
5. Re-run affected tests and the full milestone gate.
6. Commit only when the review has no unresolved blocker or high findings and
   all required gates pass.

Use a clean-context reviewer when available. A reviewer must not approve code
solely because it compiles or because PSTForge can read its own output.
Independent readers and the milestone's observable acceptance behavior are
required.

Commits must be focused and identify the target version. Do not rewrite,
squash, amend, or discard human-authored work without explicit permission.

## Architecture And Code Patterns

- Use a Cargo workspace with a thin CLI crate, a safe recovery/core crate, an
  isolated `libpff` system/FFI crate, a separately licensed PST writer crate,
  and an `xtask` automation crate.
- Keep all `unsafe` code inside the `libpff` system crate. Add
  `#![deny(unsafe_code)]` to every other crate.
- Dynamically link `libpff`; do not statically combine it with PSTForge.
- Wrap every native pointer in a single-owner RAII type. Validate nullability,
  return codes, lengths, integer conversions, and cleanup paths.
- Do not expose raw `libpff` pointers outside the FFI crate and do not send
  parser handles across threads.
- Put native parsing in supervised child processes. The supervisor owns durable
  job state and finalized output.
- Stream messages and attachments. Do not load an entire PST or unbounded
  attachment into memory.
- Use typed identifiers, byte sizes, provenance, completeness, and processing
  states. Do not encode state in filenames or free-form strings.
- Use `thiserror` for library errors and `anyhow` only at executable
  boundaries. Production code must not use `unwrap`, `expect`, `panic!`, or
  unchecked casts for recoverable conditions.
- Use `tracing` for structured diagnostics. User data, subjects, addresses,
  bodies, and attachment contents must not appear in logs by default.
- Use `serde` models with explicit schema versions for durable JSON.
- Use UTC for machine timestamps and preserve source timestamps separately.
- Use `rusqlite` with bundled SQLite for the private transactional job ledger;
  do not add a system SQLite dependency.
- Use deterministic ordering and identifiers derived from the source identity,
  configuration, and part index where the PST format permits it.
- Sanitize display names at filesystem boundaries without changing the folder
  names stored inside generated PST files.
- Prefer small interfaces around observable behavior. Add an abstraction only
  when it isolates FFI, persistent state, the PST writer, or a test substitute.

The writer crate is based on Microsoft `outlook-pst` 1.2.0 at upstream commit
`1397836e73b690dbb09663f66056012fced45ff9`. Retain its MIT notices and keep
that crate MIT-licensed. PSTForge application code is licensed
`Apache-2.0 OR MIT`. If `libpff` must be modified, keep the fork separate,
retain LGPL-3.0-or-later, publish its corresponding source when distributed,
and continue to use replaceable dynamic linking.

## Writer Specification Traceability

- Treat Microsoft Open Specifications and Microsoft MAPI documentation as the
  normative source for every PST writer structure, node type, table, property,
  flag, count, reference, and required relationship.
- Maintain `docs/WRITER_CONFORMANCE.md` as the writer's review index. Every
  implemented invariant must identify the authoritative document, revision or
  page date, exact section or property, implementation symbol, validating
  test, and independent evidence.
- Add or update the conformance entry before changing writer behavior. A
  writer change without an applicable authoritative reference is blocked until
  the contract is found or the human owner approves an explicitly empirical
  exception in the ExecPlan Decision Log.
- Do not remove, disable, or narrow completed writer output merely because its
  current implementation lacks a normative reference. Inventory it as
  undocumented, preserve the existing behavior, and present the evidence,
  compatibility impact, and options to the human owner for a decision first.
- Do not treat successful self-read, `libpff`, `pffinfo`, `readpst`, ScanPST
  repair, or Outlook tolerance as a substitute for the normative contract.
  These are independent evidence in addition to documentation.
- Audit all existing writer behavior against the conformance index before
  adding another 0.4.2 data class. Resolve undocumented or contradictory
  behavior in focused, reviewed checkpoints with ScanPST-first acceptance.

## CLI And Output Rules

- Keep the command and option contracts in `docs/PRODUCT_SPEC.md` stable.
- Send primary human or JSON command results to stdout. Send diagnostics,
  progress, and structured logs to stderr.
- Never mix human prose into JSON output.
- Do not prompt interactively. Existing incompatible output must cause a clear
  refusal, not an overwrite prompt.
- On `SIGINT` or `SIGTERM`, stop assigning work, checkpoint durable state,
  terminate workers, and exit with the documented interrupted status.
- Treat the requested part size as a hard target except for one indivisible
  message that is itself larger. Preserve that message in a marked oversize
  part and return partial-success status.
- A resume must match the source SHA-256, job schema, recovery mode, part-size
  policy, and writer format. Refuse mismatches.

## Tests And Evidence

Every parser, recovery, state, packing, writer, or CLI behavior change requires
tests proportional to its risk.

- Keep all real PST files outside the repository.
- Read the corpus location from `PSTFORGE_CORPUS_MANIFEST`; never infer or scan
  user directories for PST files.
- The external manifest records paths, SHA-256 values, classifications, and
  expected invariants. Do not commit its real paths or private metadata.
- Never mutate a corpus original. Create corrupt variants in an external
  scratch directory from a verified copy or reflink.
- Keep detailed run evidence under untracked `.agent/test-results/`; retain
  only bounded conclusions in the ExecPlan.
- Unit tests use a fake recovery backend. Integration tests use `libpff`.
  Conformance tests must also use Ubuntu/Debian `pffinfo` and independent
  `readpst`.
- PSTForge reading its own generated PST is not sufficient validation.
- The release gate includes manual import into Synology MailPlus and secondary
  Outlook compatibility checks.
- Add failure tests for truncated input, invalid indexes, malformed properties,
  huge attachments, symlinks, permission denial, disk exhaustion, worker
  crashes, forced termination, corrupt durable state, resume mismatch, and an
  item larger than the part limit.

Use `cargo xtask gate fast` during development, `cargo xtask gate full` before
a milestone commit, and `cargo xtask gate release` for release candidates.
Exact contents and expected evidence for these tiers are specified in the
ExecPlan.

On the current development host, the canonical combined external manifest is
`$HOME/.local/share/pstforge-test-corpus/full-manifest.toml`. It contains the
legacy ANSI, Unicode, damaged, and split cases plus every focused 0.4.2 case.
Before a full or release gate, verify that exact file is readable and invoke
the gate with the variable set explicitly:

```sh
test -r "$HOME/.local/share/pstforge-test-corpus/full-manifest.toml"
PSTFORGE_CORPUS_MANIFEST="$HOME/.local/share/pstforge-test-corpus/full-manifest.toml" \
  cargo xtask gate full
```

Do not substitute `$HOME/.local/share/pstforge-test-corpus/manifest.toml` or
`$HOME/.local/share/pstforge/corpus/manifest.toml` for a full gate. They are
focused manifests and intentionally omit required cases. Do not rediscover or
scan for another manifest while the canonical combined manifest is readable.

## Documentation Maintenance

The ExecPlan is a living document. Update its Progress, Surprises &
Discoveries, Decision Log, and Outcomes & Retrospective whenever work or
evidence changes the plan. Keep `docs/ROADMAP.md`, `docs/PRODUCT_SPEC.md`, and
this file consistent with it. A product behavior or milestone change is not
complete until all affected documentation agrees.
