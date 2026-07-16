# Upstream

This crate is adapted from Microsoft's `outlook-pst` crate version 1.2.0:

- Repository: <https://github.com/microsoft/outlook-pst-rs>
- Commit: `1397836e73b690dbb09663f66056012fced45ff9`
- Upstream crate path: `crates/pst`
- License: MIT

The upstream `LICENSE` is retained in this directory. PSTForge adds new-store
creation and mail-writing behavior that is not present in the pinned upstream
revision. Consult Git history for the exact adaptation diff.

PSTForge also backports Microsoft's post-1.2.0 deadlock fix from commit
`d0f9f00110990f596ea6449c078640dc5bbf294e`. The fix releases the PST reader
mutex before `root_hierarchy_table` opens the table context.
