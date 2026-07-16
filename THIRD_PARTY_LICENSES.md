# Third-Party Licensing

PSTForge application and FFI wrapper code is licensed under
`Apache-2.0 OR MIT`.

PSTForge dynamically links to the system `libpff` library. `libpff` is
distributed under `LGPL-3.0-or-later`; it is not copied, statically linked, or
modified in this repository. Operators can replace the system shared library
subject to ABI compatibility. Source and license information for `libpff` is
available from its upstream project and the operating-system package metadata.

Rust dependency license expressions are checked by `cargo xtask gate full`.
The committed `Cargo.lock` fixes the dependency set reviewed for each
milestone.

The `crates/pstforge-pst` writer is adapted from Microsoft's `outlook-pst`
1.2.0 source at commit `1397836e73b690dbb09663f66056012fced45ff9` and remains
MIT-licensed. Its Microsoft copyright notice and full MIT license are retained
in `crates/pstforge-pst/LICENSE`; provenance and the pinned revision are in
`crates/pstforge-pst/UPSTREAM.md`.
