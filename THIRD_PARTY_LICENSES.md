# Third-Party Licensing

PSTForge application and FFI wrapper code is licensed under
`Apache-2.0 OR MIT`.

PSTForge dynamically links to a private build of the separately maintained
`calculatetech/libpff` fork. `libpff` remains `LGPL-3.0-or-later`, is pinned as
the `third_party/libpff` submodule, and is not statically combined with
PSTForge. Debian packages install the replaceable shared object, its exact
revision, complete corresponding source, LGPL notices, and build instructions.

Rust dependency license expressions are checked by `cargo xtask gate full`.
The committed `Cargo.lock` fixes the dependency set reviewed for each
milestone. Debian packages additionally install
`RUST-DEPENDENCY-LICENSES.txt`, generated from the locked executable dependency
closure and the complete license/notice files shipped by each crate.

The `crates/pstforge-pst` writer is adapted from Microsoft's `outlook-pst`
1.2.0 source at commit `1397836e73b690dbb09663f66056012fced45ff9` and remains
MIT-licensed. Its Microsoft copyright notice and full MIT license are retained
in `crates/pstforge-pst/LICENSE`; provenance and the pinned revision are in
`crates/pstforge-pst/UPSTREAM.md`.
