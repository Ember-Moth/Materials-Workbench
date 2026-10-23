# Projects of older format versions

One project per released format version (`docs/design/project-format.md` §4), read by `tests/migration.rs`: the current code must open each one, find the content it held, and back it up on the first save.

| Directory | Content |
|---|---|
| `v1/tio2.mwproj` | The `TiO₂ 相研究` sample (`samples/tio2.mwproj`) exactly as format version 1 wrote it (commit 99e843d): rutile imported from `rutile.cif`, a hand-written test fixture of `crates/mw-crystal-io/tests/fixtures/` with commonly cited literature parameters, and a 2 × 2 × 2 supercell derived from it |

These files are frozen: never regenerate or edit them. When the format version goes up, copy the current sample here as the new version's test project before regenerating the samples.
