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
