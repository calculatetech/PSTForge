# PSTForge

[![CI](https://github.com/calculatetech/PSTForge/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/calculatetech/PSTForge/actions/workflows/ci.yml)
[![Security Audit](https://github.com/calculatetech/PSTForge/actions/workflows/security.yml/badge.svg)](https://github.com/calculatetech/PSTForge/actions/workflows/security.yml)

PSTForge is a Linux-native command-line utility for recovering large or damaged
Outlook PST files without modifying the source. It writes new, independently
importable Unicode PST files, normally capped at 4 GiB, for checkpointed import
into Synology MailPlus Server. Clean ScanPST analysis followed by successful
Microsoft Outlook inspection is the required interoperability acceptance
boundary; MailPlus compatibility is then assumed.

PSTForge 1.0 uses a stable command and report contract. The output writer and
recovery pipeline have been validated with real damaged PSTs, ScanPST, Outlook,
MailPlus, `libpff`, `pffinfo`, and `readpst`. MailPlus import is optional
operator evidence rather than a release gate; a MailPlus-only failure after
ScanPST and Outlook pass belongs in a Synology support ticket.

## Features

- Opens the source read-only, refuses source symlinks and unsafe output paths,
  and rechecks source identity before reporting completion.
- Reads healthy and damaged ANSI and Unicode PST input supported by `libpff`.
- Preserves readable mail, contacts, distribution lists, appointments,
  meetings, tasks, notes, documents, unknown message classes, recipients,
  attachments, recursively embedded messages, named/raw properties, folder and
  store metadata, and hidden associated items.
- Recovers reachable, deleted, recovered, and orphan items in balanced mode;
  aggressive mode also asks `libpff` to ignore allocation data and scan for
  fragments.
- Writes complete Unicode PST parts with the original source folder layout.
  The configured size is a hard target unless one indivisible item is larger.
- Streams directly by default: one source traversal, no payload spool, no
  completed-part hash pass, and atomic publication of each finished part.
- Offers opt-in restartable recovery with durable payload state and strict
  source/configuration matching on resume.
- Produces bounded `recovery.log` accounting and versioned human or JSON
  reports without logging message subjects, addresses, bodies, or attachment
  contents.
- Provides read-only `info`, streaming `verify`, durable-spool `recover`, PST
  producing `split`, and persisted-job `report` commands.

## Limitations

- PSTForge can recover only data that remains readable through `libpff` and can
  be represented safely in a PST. Omitted or damaged content is counted by
  reason; it is never silently reported as written.
- Direct mode is intentionally not resumable. An interrupted direct job keeps
  finalized parts and reporting state, but a retry must use a new empty output
  directory.
- Restartable mode adds approximately one complete readable-payload write and
  requires conservative free space of three times the source size. It also
  requires `pffinfo` and `readpst`; direct mode does not.
- Direct mode retains bounded control metadata for one top-level item graph.
  Its supervisor limit is 256 MiB and 262,144 frames; attachment/property bytes
  themselves are streamed.
- A single item larger than `--max-pst-size` is preserved alone in a marked
  oversize part and produces partial-success status.
- Existing output is never overwritten. A nonempty new-job directory or a
  conflicting `part-NNNN.pst` filename causes a refusal.
- Mail clients do not expose every native PST class or embedded object in the
  same way. A client display/import limitation does not imply the underlying
  PST object was omitted; use the reports and an independent reader when
  investigating a discrepancy.
- PSTForge 1.0 does not include in-place repair, PST merging, OST conversion,
  password cracking, general attachment export, EML/Maildir/PDF output, a GUI,
  date-range splitting, or folder-based splitting.

## Installation

### Debian package

Build one package with one release compilation, then install it with APT so
runtime dependencies are resolved:

```bash
cargo xtask package deb
sudo apt install ./target/debian/pstforge_1.0.0_amd64.deb
pstforge --version
```

The default command produces one `.deb` and does not perform a second build or
simulate an installation. Maintainers can opt into reproducibility, package
contents, linkage, lintian, and isolated install/removal checks:

```bash
cargo xtask package deb --validate
```

The package targets `amd64` and dynamically links the replaceable system
`libpff.so.1`. It installs the binary, manpage, product documentation, public
JSON schemas, and all applicable license notices. Remove the program without
touching source PSTs or recovery jobs:

```bash
sudo apt remove pstforge
```

### Compile from source

The minimum supported Rust version is 1.85. On Ubuntu 26.04, the verified build
packages are:

```bash
sudo apt update
sudo apt install build-essential pkg-config cargo rustc libpff-dev
cargo build --locked --release -p pstforge-cli
./target/release/pstforge --version
```

To install that source build outside the package manager:

```bash
sudo install -Dm755 target/release/pstforge /usr/local/bin/pstforge
```

Development and full acceptance use these additional verified Ubuntu packages:

```bash
sudo apt install rustfmt rust-clippy pff-tools pst-utils dpkg-dev lintian \
  binutils gzip findutils coreutils
cargo install cargo-audit --locked
```

`libpff-dev` supplies the dynamically linked parser headers/library,
`pff-tools` supplies `pffinfo`, and `pst-utils` supplies `readpst`. The Debian
builder additionally uses `dpkg-dev`, `lintian`, `binutils`, `gzip`, `findutils`,
and `coreutils`.

## Basic Usage

Inspect a source without traversing its items:

```bash
pstforge info /storage/damaged.pst
```

Account for reachable content, or include balanced recovery collections:

```bash
pstforge verify /storage/damaged.pst --mode full
pstforge verify /storage/damaged.pst --mode recovery --json > verify.json
```

Split directly using the default 4 GiB target:

```bash
pstforge split /storage/damaged.pst --output /recovery/job-001
```

Use another hard part target or aggressive recovery:

```bash
pstforge split /storage/damaged.pst \
  --output /recovery/job-002 \
  --max-pst-size 2GiB \
  --recovery aggressive
```

Choose restartability deliberately when its disk/write cost is acceptable:

```bash
pstforge split /storage/damaged.pst \
  --output /recovery/job-003 \
  --restartable

pstforge split /storage/damaged.pst \
  --output /recovery/job-003 \
  --restartable \
  --resume
```

`--keep-work` retains restartable payload state after success. Without it,
payload storage is removed after finalized parts and accounting are durable.

Create canonical restartable recovery state without writing PST parts, or
recreate a report from an existing split job:

```bash
pstforge recover /storage/damaged.pst --output /recovery/spool-only
pstforge report /recovery/job-001
pstforge report /recovery/job-001 --json > report.json
```

Final command results go to stdout. Progress and diagnostics go to stderr.
`--json` selects one versioned JSON result; `--log-format json` separately
selects structured stderr diagnostics. All commands also accept `--quiet`,
`--color auto|always|never`, and repeatable `-v`.

## Output

A split job contains:

```text
job-001/
  parts/part-0001.pst
  recovery.log
  .pstforge/manifests/part-0001.json
```

Parts are created beside their destination under temporary names, flushed, and
atomically renamed only after writer validation. `.pstforge` is private durable
state. Restartable payload data exists there only when `--restartable` is used.
For a new job, PSTForge creates a missing output directory and its missing
parent directories with private permissions; symlinked path components are
refused. Do not place the source PST inside the job directory.

Public JSON schemas are in [`docs/schemas`](docs/schemas) and install to
`/usr/share/pstforge/schemas`. `recovery.log` is intentionally aggregate and
bounded; detailed private test/run evidence should be kept separately.

## Exit Status

| Status | Meaning |
| ---: | --- |
| `0` | Complete success |
| `1` | Partial success with usable, explicitly accounted output |
| `2` | Invalid command-line usage |
| `3` | Source or parser failure |
| `4` | Output or durable-state failure |
| `5` | Generated PST conformance failure |
| `6` | Internal or supervision failure |
| `130` | Interrupted by `SIGINT` or `SIGTERM` |

Automation must treat status `1` as requiring review of `recovery.log` and the
JSON report, not as complete success.

## Testing

Fast tests use generated fixtures and do not need private PST data:

```bash
cargo xtask gate fast
cargo xtask gate ci
```

Real PST files must live outside the repository and are referenced only by an
explicit manifest. On the established development host, run the full gate with
the canonical combined manifest:

```bash
test -r "$HOME/.local/share/pstforge-test-corpus/full-manifest.toml"
PSTFORGE_CORPUS_MANIFEST="$HOME/.local/share/pstforge-test-corpus/full-manifest.toml" \
  cargo xtask gate full
```

Start a new external manifest from
[`tests/corpus-manifest.example.toml`](tests/corpus-manifest.example.toml).
PSTForge never scans user directories to discover test PSTs. Detailed gate
evidence is written beneath the ignored `.agent/test-results/` directory, and
independent-reader output is redacted because it can contain mailbox data.

GitHub runs the public CI gate on pull requests and pushes to `main` in Ubuntu
24.04 and Debian 13. Protected `main` requires both jobs and resolved
review threads before merge. Scheduled jobs run RustSec and a bounded parser
fuzz target. Private-corpus `full` and `release` gates run only on the local
machine holding the explicitly configured corpus; PST files, paths, reader
output, and detailed evidence are never uploaded to GitHub. Changes that can
affect output PST bytes or recovered content also require a focused
ScanPST-first human gate followed by Outlook inspection. MailPlus import is not
an external acceptance dependency. Documentation, automation, packaging,
reporting, and proven byte-identical refactors do not require the human gate.
Release automation accepts only the existing tag matching the package version,
requires approval through the `release` environment, and retains a Debian
build artifact without creating a GitHub release.

See [`docs/PRODUCT_SPEC.md`](docs/PRODUCT_SPEC.md) for the authoritative
behavior contract, [`docs/ATTACHMENT_RECOVERY.md`](docs/ATTACHMENT_RECOVERY.md)
for attachment-recovery confidence, and [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md)
for licensing and upstream provenance.
