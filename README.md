# PSTForge

PSTForge is a Linux command-line utility for read-only inspection and eventual
recovery of large or damaged Outlook PST files. Version 0.1.0 provides source
identity protection, metadata inspection, and reachable folder/message counts.

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
`verify` additionally traverses reachable folders and counts their messages.

## Local Gates

```bash
cargo xtask gate fast
PSTFORGE_CORPUS_MANIFEST=/absolute/external/manifest.toml \
  cargo xtask gate full
```

Real PST files and their manifest must remain outside the repository. Start
from [`tests/corpus-manifest.example.toml`](tests/corpus-manifest.example.toml).
The full gate verifies source hash and timestamps before and after both CLI
commands, then reads each healthy milestone case with `pffinfo` and `readpst`.
Detailed logs are written under the ignored `.agent/test-results/` directory;
independent-reader output is redacted because it can contain mailbox data.
