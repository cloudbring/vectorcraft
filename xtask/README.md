# VectorCraft workspace tasks

`cargo xtask` retains the app's task list and adds shared syntax from `cli_help()`.
`craft-xtask` and its existing hashing backend `craft-hash` are tooling-only dependencies,
locked to craft-libs integration commit `89133cd7c5d061ebd6ffcb933dd786c42794f07c`.

`src/config.rs` owns VectorCraft's layer assignments, within-layer exceptions, wasm selection
and seven immutable PDF package pins. Version and statistics dispatch to the shared `run`
functions. Layers use shared `from_metadata()`/`check()` with the existing table formatter.
PDF uses shared `validate()` with the existing success line; ICO uses shared `pack_ico()`
with the existing silent CLI. Their shared `run()` functions print different success text,
so using those directly would break the captured output contract. Byte-buffer hash callers
use `craft-hash` directly to hash the same bytes they verify/copy, rather than rereading a file;
Affinity corpus file verification uses `craft_xtask::sha256_hex()`.

Assets, brands, cursors, bundles, corpora, web fonts, i18n, CI, test/doctest and wasm orchestration
stay app-owned. There was no parity task; no unrelated parity policy is introduced.

## Behavior boundaries

- Successful layers, PDF checks, ICO bytes/silence, version stdout/edits and statistics reports
  reproduce the original golden fixtures. Root help retains its original text and appends
  shared syntax. `version tag` is now available through the shared version dispatcher.
- Shared error diagnostics replace the old layer/PDF/version errors. Statistics now rejects
  unsupported arguments rather than ignoring them. Version lockfile refresh is captured by
  the library instead of echoing Cargo's progress to stderr.
- ICO now rejects empty, zero-sized and nonsquare images rather than emitting malformed ICOs.
- Statistics follows symlinks as before but deduplicates canonical directories to break cycles,
  with the library's two-million-entry/ten-thousand-directory limits. Alias directories no
  longer double-count files. Ordinary tree counts and exclusions are unchanged.
- The shared layer metadata reader treats nonmember path dependencies as external rather than
  workspace crates, and rejects edges to unregistered crates. Neither affects the current tree.

Only the xtask crate needs to be built/tested for this migration. Run `cargo test -p xtask`,
`cargo clippy -p xtask --all-targets -- -D warnings`, `cargo fmt --check -p xtask`,
`cargo xtask layers` and `cargo xtask pdf-deps`, with an isolated `CARGO_TARGET_DIR`.
The `stats --exact`, `test`, `ci`, `bundle` and `wasm` tasks intentionally build app crates;
do not run them for tooling-only verification.
