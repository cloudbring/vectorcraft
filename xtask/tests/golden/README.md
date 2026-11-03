# Ticket #70: original VectorCraft outputs

Captured on 2026-11-03 **before deleting the original implementations**, from pristine
VectorCraft HEAD `11862f61198c8f62c0b22b6c9a331cc5950ac98d` in the supplied worktree.
The complete fixture directory is below 200 KB. No Adobe software, files or implementation
sources were used. The PNG hex fixtures encode original, programmatically generated
transparent square images, not product artwork or distributable app assets.

- `layers.stdout`, `pdf-deps.stdout`, `stats.stdout`, `help.stdout`: the original xtask
  binary on the current repository, built with `cargo build -p xtask` only.
- `metadata.json`: the same tree's `cargo metadata --no-deps`, reduced to package IDs,
  names, membership and dependencies, with checkout paths replaced by `/workspace`.
- `stats-counts.json`: the original report's per-package counts. The test reconstructs
  a temporary source tree with those counts to replay the full report byte-for-byte,
  including xtask's old counts without retaining the deleted task implementations.
- `version-before.toml`: a dependency-free temporary workspace using the current version
  (`0.21.0`), edition and workspace inheritance. The original `version.rs` was compiled
  into a temporary harness whose Cargo cwd was that workspace. `version.stdout`,
  `version-set.stdout`, `version-after.toml` and `version-after.lock` capture a read and
  a real offline lockfile update to `1.2.3-rc.1`. The production workspace was not changed.
- `pngs.json`: PNG hex for sizes 16, 32, 48, 128 and 256; `ico.hex` and the empty
  `ico.stdout`: the original CLI's packed output and silence, in the same input order.
- `sha256.stdout`: the original helper's hashes of empty bytes, `abc`, `font bytes`,
  `png` and 1000 ASCII `a` bytes. Both shared byte and streaming file APIs are compared.

Live dispatcher tests also compare layers/PDF success output to these original fixtures.
Policy tests reject forbidden layer edges and PDF pin drift without compiling the app.
