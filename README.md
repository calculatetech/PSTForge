# PSTForge

PSTForge is a Linux command-line utility for read-only inspection and eventual
recovery of large or damaged Outlook PST files. Version 0.2.0 adds the internal
Unicode PST writer foundation to the existing protected source inspection and
complete reachable-mail inventory.

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
```

PSTForge refuses source symlinks and opens the source with Linux read-only,
no-follow, no-atime flags. `info` hashes the file and reports format metadata.
`verify` additionally streams the reachable mail catalog, including recipients,
bodies, raw properties, attachments, and embedded-message relationships. It
reports byte totals and the peak stream chunk without retaining an unbounded
property or attachment in memory.

## Local Gates

```bash
cargo xtask gate fast
PSTFORGE_CORPUS_MANIFEST=/absolute/external/manifest.toml \
  cargo xtask gate full
```

Real PST files and their manifest must remain outside the repository. Start
from [`tests/corpus-manifest.example.toml`](tests/corpus-manifest.example.toml).
The full gate verifies source hash and timestamps before and after both CLI
commands, creates a fresh one-folder/one-message Unicode PST without a runtime
template, and reads generated and healthy corpus cases with `pffinfo` and
`readpst`.
Detailed logs are written under the ignored `.agent/test-results/` directory;
independent-reader output is redacted because it can contain mailbox data.

Writer developers can generate the 0.2.0 acceptance store directly:

```bash
cargo run -p pstforge-pst --example create_minimal -- /tmp/pstforge-smoke.pst
```
