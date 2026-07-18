# PSTForge

PSTForge is a Linux command-line utility for recovery of large or damaged
Outlook PST files. Version 0.3.0 adds balanced normal, recovered, and orphan
mail extraction into a private transactional job spool. The accepted 0.2.1
writer provides the template-free mail-fidelity PST structures used by later
size-limited packing milestones.

## Ubuntu Dependencies

The verified Ubuntu package names for development and local acceptance are:

```bash
sudo apt install build-essential pkg-config cargo rustc rustfmt rust-clippy \
  libpff-dev pff-tools pst-utils
cargo install cargo-audit --locked
```

`libpff-dev` supplies the dynamically linked parser and headers. `pff-tools`
supplies `pffinfo`; `pst-utils` supplies the independent `readpst` validator.
The project MSRV is Rust 1.85.

## Usage

```bash
cargo run -p pstforge-cli -- info /data/mail.pst
cargo run -p pstforge-cli -- info /data/mail.pst --json
cargo run -p pstforge-cli -- verify /data/mail.pst --mode full
cargo run -p pstforge-cli -- recover /data/mail.pst --output /data/recovery-job
```

PSTForge refuses source symlinks and opens the source with Linux read-only,
no-follow, no-atime flags. `info` hashes the file and reports format metadata.
`verify` additionally streams the reachable mail catalog, including recipients,
bodies, raw properties, attachments, and embedded-message relationships. It
reports byte totals and the peak stream chunk without retaining an unbounded
property or attachment in memory.

`recover` traverses reachable mail first, invokes balanced `libpff` recovery,
then spools recovered and orphan candidates. It creates a private SQLite ledger
and content-addressed blobs under `OUTPUT/.pstforge`; it does not yet create
importable PST parts. A fresh run refuses a nonempty output directory. Exit
status `1` means durable candidates were produced but some source content was
partial or damaged.

## Local Gates

```bash
cargo xtask gate fast
PSTFORGE_CORPUS_MANIFEST=/absolute/external/manifest.toml \
  cargo xtask gate full
```

Real PST files and their manifest must remain outside the repository. Start
from [`tests/corpus-manifest.example.toml`](tests/corpus-manifest.example.toml).
The full gate verifies source hash and timestamps before and after inspection
and recovery commands, creates a rich Unicode PST without a runtime template, round-trips it
through `libpff`, `pffinfo`, and independent `readpst`, and validates healthy
external corpus cases.
Detailed logs are written under the ignored `.agent/test-results/` directory;
independent-reader output is redacted because it can contain mailbox data.
The release-facing [attachment recovery matrix](docs/ATTACHMENT_RECOVERY.md)
documents which missing MIME types can be proven from complete payload bytes,
which damaged containers receive only a generic label, and which unknown data
is retained for later analysis.

Writer developers can generate the 0.2.1 acceptance store directly:

```bash
cargo run -p pstforge-pst --example create_fidelity -- /tmp/pstforge-fidelity.pst
```

The current public writer boundary emits one deterministic mail folder and one
top-level message per call. It supports multiple To/Cc/Bcc recipients,
by-value attachments, inline content metadata, custom named-property GUID
sets, typed raw properties, and one attachment level of embedded messages.
Version 0.2.1 externalizes individual property payloads above the heap limit
through 16 KiB and rejects larger values before publication; the 0.4.x packer
removes that bounded-fixture limit as part of arbitrary size-limited output.
The
0.3.x pipeline supplies recovered canonical items; 0.4.x generalizes folder
and message packing across size-limited parts.
